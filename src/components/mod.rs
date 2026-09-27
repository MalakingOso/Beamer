//! Installs the local extraction server (llama.cpp runtime, preset, launchers,
//! Scheduled Task, model) that `llm/` talks to. Everything is declared in a
//! catalog compiled into the exe, and at launch the running exe reconciles
//! the disk to *its own* catalog, so version N always ends up with what N
//! expects however it was installed. No manifest is fetched, so exe and
//! catalog cannot disagree. Windows on ARM64 only; the catalog is empty elsewhere.
//!
//! `plan` (here) is the pure diff; `reconcile` stages and applies it via
//! `apply`; `state` records what was applied (`components.json`). The only
//! code allowed to touch the server's lifecycle. Not under `src/llm/` because
//! it needs crate-rooted paths. See `agent_docs/local_inference.md`.

mod apply;
mod archive;
mod fetch;
mod llama;
mod reconcile;
mod state;

use std::path::PathBuf;

use sha2::{Digest, Sha256};

pub use reconcile::{use_components, Components, Status};
pub use state::ComponentStore;

/// Members of a group are applied all-or-nothing, between the group's stop
/// and start hooks (see `apply`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    /// The local extraction server: runtime, preset, launchers, Scheduled
    /// Task and model. The server is stopped for the swap and started after,
    /// and never before the model it names exists.
    LlamaServer,
}

impl Group {
    /// Status key for group-wide outcomes (an apply that failed as a whole),
    /// so they show as one row rather than one per member.
    pub fn id(self) -> &'static str {
        match self {
            Group::LlamaServer => "llama-server",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Group::LlamaServer => "Local AI",
        }
    }
}

/// Where a component lives. A closed set, never a free-form path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Root {
    /// `%LOCALAPPDATA%\Beamer` (`~/.local/share/Beamer` on Linux). Not the
    /// install dir: Beamer runs asInvoker and must be able to write here.
    LocalData,
    /// `~/models/beamer`, shared with hand-placed models on other machines.
    Models,
}

impl Root {
    pub fn dir(self) -> PathBuf {
        match self {
            Root::LocalData => dirs::data_local_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("Beamer"),
            Root::Models => dirs::home_dir()
                .unwrap_or_default()
                .join("models")
                .join("beamer"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Kind {
    /// Text compiled into the exe, always written LF so the bytes (and hash)
    /// don't depend on whether the building checkout converted to CRLF. LF
    /// because that is what the server was validated with; nothing proves the
    /// fork's ini parser strips a trailing `\r` from a section name or value.
    Text(&'static str),
    /// One file, downloaded and sha256-verified.
    File { url: &'static str, sha256: &'static str, size: u64 },
    /// A zip whose contents become the directory at `path`, swapped in whole.
    Archive { url: &'static str, sha256: &'static str, size: u64 },
    /// The Scheduled Task that runs `launcher` (a path under the same root)
    /// at logon. Its definition XML is rendered at runtime (it carries the
    /// absolute launcher path) and written to `path` before registering.
    Task { name: &'static str, launcher: &'static str },
}

#[derive(Debug)]
pub struct Component {
    pub id: &'static str,
    /// Shown in Settings.
    pub label: &'static str,
    /// Human label only, recorded for logs. Freshness is decided by sha256.
    pub version: &'static str,
    pub kind: Kind,
    pub root: Root,
    /// `/`-separated, relative to `root`.
    pub path: &'static str,
    pub group: Option<Group>,
    /// Needs the user's OK before downloading (big files). Auto-accepted on a
    /// fresh install, and only asked when a download is actually needed: a
    /// file already present that verifies is never prompted for.
    pub prompt: bool,
}

impl Component {
    pub fn dest(&self) -> PathBuf {
        self.path.split('/').fold(self.root.dir(), |p, part| p.join(part))
    }

    /// The exact bytes to write, for kinds that carry their own content.
    pub fn content(&self) -> Option<Vec<u8>> {
        match self.kind {
            Kind::Text(text) => Some(text.replace("\r\n", "\n").into_bytes()),
            Kind::Task { launcher, .. } => {
                let launcher = launcher.split('/').fold(self.root.dir(), |p, part| p.join(part));
                Some(llama::task_xml(&launcher, &llama::current_user()))
            }
            Kind::File { .. } | Kind::Archive { .. } => None,
        }
    }

    /// Lowercase hex sha256 of what should be on disk (for an archive, of the
    /// zip it was extracted from).
    pub fn sha256(&self) -> String {
        match self.kind {
            Kind::File { sha256, .. } | Kind::Archive { sha256, .. } => sha256.to_ascii_lowercase(),
            _ => sha256_hex(&self.content().unwrap_or_default()),
        }
    }

    pub fn download_size(&self) -> Option<u64> {
        match self.kind {
            Kind::File { size, .. } | Kind::Archive { size, .. } => Some(size),
            _ => None,
        }
    }

    /// The pinned apply order within a group: the archive first (it replaces
    /// the whole directory the text files live in), then those files, then the
    /// model, then the task registration.
    fn apply_rank(&self) -> u8 {
        match self.kind {
            Kind::Archive { .. } => 0,
            Kind::Text(_) => 1,
            Kind::File { .. } => 2,
            Kind::Task { .. } => 3,
        }
    }

    /// True when `self` lives inside `dir`'s directory, so swapping `dir`
    /// in replaces it.
    fn is_inside(&self, dir: &Component) -> bool {
        self.root == dir.root
            && self.path.strip_prefix(dir.path).is_some_and(|rest| rest.starts_with('/'))
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The Settings model line (`local_ai_card`) follows this component.
pub const MODEL_ID: &str = "k2h-model";

const RUNTIME_DIR: &str = "llama-k2horizon";

/// The K2-Horizon local extraction server (Windows on ARM64 only: the fork's
/// `llama-server.exe` is built for nothing else). Keep the model's filename
/// stem, the preset ini's `[section]` and `llm::default_extract_model()` in
/// step (`tests.rs` pins them).
static LLAMA_SERVER: &[Component] = &[
    Component {
        id: "k2h-runtime",
        label: "Local AI runtime",
        // `deploy/publish-llama-runtime.sh <N>` prints this entry; its upload
        // step must have run before a build carrying it ships.
        version: "k2h-1",
        kind: Kind::Archive {
            url: "https://github.com/MalakingOso/Beamer/releases/download/llama-runtime-k2h-1/llama-runtime-k2h-1-aarch64-pc-windows-msvc.zip",
            sha256: "5b2ff20d1cf99b0b2b84c91b6b6be613532c48290a31d43ae9b2aa976ce2c968",
            size: 8_377_666,
        },
        root: Root::LocalData,
        path: RUNTIME_DIR,
        group: Some(Group::LlamaServer),
        prompt: false,
    },
    // The ini and both launchers share the runtime dir: the .cmd finds the
    // ini and llama-server.exe via `%~dp0`.
    Component {
        id: "k2h-preset",
        label: "Local AI preset",
        version: "embedded",
        kind: Kind::Text(include_str!("../../deploy/llama-models-bearcave.ini")),
        root: Root::LocalData,
        path: "llama-k2horizon/llama-models-bearcave.ini",
        group: Some(Group::LlamaServer),
        prompt: false,
    },
    Component {
        id: "k2h-launcher",
        label: "Local AI launcher",
        version: "embedded",
        kind: Kind::Text(include_str!("../../installer/k2horizon/start-llama-k2horizon.cmd")),
        root: Root::LocalData,
        path: "llama-k2horizon/start-llama-k2horizon.cmd",
        group: Some(Group::LlamaServer),
        prompt: false,
    },
    Component {
        id: "k2h-launcher-hidden",
        label: "Local AI launcher",
        version: "embedded",
        kind: Kind::Text(include_str!("../../installer/k2horizon/start-llama-k2horizon-hidden.vbs")),
        root: Root::LocalData,
        path: "llama-k2horizon/start-llama-k2horizon-hidden.vbs",
        group: Some(Group::LlamaServer),
        prompt: false,
    },
    Component {
        id: MODEL_ID,
        label: "K2-Horizon model",
        // If swapped, re-check url, sha256 and size together against `GET
        // https://huggingface.co/api/models/NANI-Nithin/K2-Horizon-0.9B-GGUF?blobs=true`.
        version: "K2-Horizon-0.9B-Q8_0",
        kind: Kind::File {
            url: "https://huggingface.co/NANI-Nithin/K2-Horizon-0.9B-GGUF/resolve/main/K2-Horizon-0.9B-Q8_0.gguf",
            sha256: "741fb9ad263956c003883cb7caddeb14b4dfb9b4b67b5414ad12967e965dd547",
            size: 1_148_614_016,
        },
        root: Root::Models,
        // `installer/k2horizon/hooks.nsh`'s uninstall section deletes this
        // exact filename (and its `.part`).
        path: "K2-Horizon-0.9B-Q8_0.gguf",
        group: Some(Group::LlamaServer),
        prompt: true,
    },
    Component {
        id: "k2h-task",
        label: "Local AI startup task",
        version: "embedded",
        kind: Kind::Task {
            name: llama::TASK_NAME,
            launcher: "llama-k2horizon/start-llama-k2horizon-hidden.vbs",
        },
        root: Root::LocalData,
        path: "llama-k2horizon/task.xml",
        group: Some(Group::LlamaServer),
        prompt: false,
    },
];

/// This build's catalog. `cfg!`, not `#[cfg]`: every entry and code path
/// still compiles (and counts as used) on every target, only the selection
/// differs, so no target trips `-D warnings` on dead code.
pub fn catalog() -> &'static [Component] {
    if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        LLAMA_SERVER
    } else {
        &[]
    }
}

pub fn find(id: &str) -> Option<&'static Component> {
    LLAMA_SERVER.iter().find(|c| c.id == id)
}

/// What a probe of the destination found. Only content kinds (`Text`,
/// `Task`) are compared byte-for-byte; a download is never re-hashed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnDisk {
    Missing,
    Present,
    Differs,
}

/// One unit of work: the stale members of a group (or a single ungrouped
/// component), in apply order.
#[derive(Debug)]
pub struct GroupPlan {
    pub group: Option<Group>,
    pub members: Vec<&'static Component>,
}

/// The diff. A component is current when the destination is present (and,
/// for content kinds, byte-identical) *and* `components.json` records the
/// sha256 this catalog expects. A group with any stale member also takes:
/// every member inside a stale archive's directory (the swap removes them),
/// and its task (re-registering is idempotent, and keeps a deleted task from
/// wedging every later start).
pub fn plan(
    catalog: &'static [Component],
    state: &ComponentStore,
    probe: impl Fn(&Component) -> OnDisk,
) -> Vec<GroupPlan> {
    let is_current = |c: &Component| {
        probe(c) == OnDisk::Present
            && state.get(c.id).is_some_and(|r| r.sha256.eq_ignore_ascii_case(&c.sha256()))
    };
    let stale: Vec<&'static Component> = catalog.iter().filter(|c| !is_current(c)).collect();

    let mut plans: Vec<GroupPlan> = Vec::new();
    for c in &stale {
        let Some(group) = c.group else {
            plans.push(GroupPlan { group: None, members: vec![*c] });
            continue;
        };
        if plans.iter().any(|p| p.group == Some(group)) {
            continue;
        }
        let swapped: Vec<&Component> = stale
            .iter()
            .filter(|s| s.group == Some(group) && matches!(s.kind, Kind::Archive { .. }))
            .copied()
            .collect();
        let mut members: Vec<&'static Component> = catalog
            .iter()
            .filter(|m| m.group == Some(group))
            .filter(|m| {
                stale.iter().any(|s| s.id == m.id)
                    || swapped.iter().any(|a| m.is_inside(a))
                    || matches!(m.kind, Kind::Task { .. })
            })
            .collect();
        members.sort_by_key(|m| m.apply_rank());
        plans.push(GroupPlan { group: Some(group), members });
    }
    plans
}

#[cfg(test)]
mod tests;
