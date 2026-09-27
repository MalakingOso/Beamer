//! Tests for `plan` (staleness, group expansion, apply order) against a toy
//! catalog, then consistency checks on the real one: the model's filename
//! stem must match the preset's `[section]` and (on aarch64) the default
//! extraction model, and the launchers, preset and task must land where each
//! other expects.

use std::collections::HashMap;

use super::state::Record;
use super::*;

const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SHA_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

const fn member(id: &'static str, kind: Kind, root: Root, path: &'static str, prompt: bool) -> Component {
    Component { id, label: id, version: "v", kind, root, path, group: Some(Group::LlamaServer), prompt }
}

/// Deliberately out of apply order, to prove `plan` sorts.
static TEST: &[Component] = &[
    member("task", Kind::Task { name: "t", launcher: "rt/run.vbs" }, Root::LocalData, "rt/task.xml", false),
    member("model", Kind::File { url: "u", sha256: SHA_A, size: 1 }, Root::Models, "m.gguf", true),
    member("ini", Kind::Text("[m]\n"), Root::LocalData, "rt/preset.ini", false),
    member("runtime", Kind::Archive { url: "u", sha256: SHA_B, size: 1 }, Root::LocalData, "rt", false),
    // Same prefix, but a sibling of `rt`, not inside it.
    member("rtx", Kind::Text("x"), Root::LocalData, "rtx/other.ini", false),
];

static UNGROUPED: &[Component] = &[Component {
    id: "solo",
    label: "solo",
    version: "v",
    kind: Kind::Text("solo"),
    root: Root::LocalData,
    path: "solo.txt",
    group: None,
    prompt: false,
}];

/// Everything recorded at the sha this catalog expects.
fn all_recorded(catalog: &'static [Component]) -> ComponentStore {
    let mut store = ComponentStore::in_memory();
    for c in catalog {
        store.set(c.id, Record { version: "v".into(), sha256: c.sha256() });
    }
    store
}

fn ids(plans: &[GroupPlan]) -> Vec<Vec<&'static str>> {
    plans.iter().map(|p| p.members.iter().map(|m| m.id).collect()).collect()
}

fn on_disk(overrides: &[(&'static str, OnDisk)]) -> impl Fn(&Component) -> OnDisk {
    let map: HashMap<&str, OnDisk> = overrides.iter().copied().collect();
    move |c: &Component| map.get(c.id).copied().unwrap_or(OnDisk::Present)
}

#[test]
fn a_fresh_machine_plans_the_whole_group_in_apply_order() {
    let plans = plan(TEST, &ComponentStore::in_memory(), |_| OnDisk::Missing);
    assert_eq!(ids(&plans), vec![vec!["runtime", "ini", "rtx", "model", "task"]]);
    assert_eq!(plans[0].group, Some(Group::LlamaServer));
}

#[test]
fn everything_recorded_and_present_plans_nothing() {
    assert!(plan(TEST, &all_recorded(TEST), on_disk(&[])).is_empty());
}

#[test]
fn a_missing_model_brings_only_itself_and_the_task() {
    let plans = plan(TEST, &all_recorded(TEST), on_disk(&[("model", OnDisk::Missing)]));
    assert_eq!(ids(&plans), vec![vec!["model", "task"]]);
}

#[test]
fn a_stale_archive_brings_every_file_inside_its_dir_but_not_siblings() {
    let mut state = all_recorded(TEST);
    state.set("runtime", Record { version: "old".into(), sha256: SHA_A.into() });
    let plans = plan(TEST, &state, on_disk(&[]));
    assert_eq!(
        ids(&plans),
        vec![vec!["runtime", "ini", "task"]],
        "the swap deletes `ini` and `task.xml`, so both are rewritten; `rtx` is a sibling"
    );
}

#[test]
fn a_hand_edited_text_file_is_rewritten_despite_its_record() {
    let plans = plan(TEST, &all_recorded(TEST), on_disk(&[("ini", OnDisk::Differs)]));
    assert_eq!(ids(&plans), vec![vec!["ini", "task"]]);
}

#[test]
fn present_but_unrecorded_is_stale_which_is_the_one_time_migration() {
    let plans = plan(TEST, &ComponentStore::in_memory(), on_disk(&[]));
    assert_eq!(ids(&plans), vec![vec!["runtime", "ini", "rtx", "model", "task"]]);
}

#[test]
fn a_recorded_sha_that_no_longer_matches_the_catalog_is_stale() {
    let mut state = all_recorded(TEST);
    state.set("model", Record { version: "old".into(), sha256: SHA_B.into() });
    assert_eq!(ids(&plan(TEST, &state, on_disk(&[]))), vec![vec!["model", "task"]]);
}

#[test]
fn recorded_sha_comparison_ignores_case() {
    let mut state = all_recorded(TEST);
    state.set("model", Record { version: "v".into(), sha256: SHA_A.to_ascii_uppercase() });
    assert!(plan(TEST, &state, on_disk(&[])).is_empty());
}

#[test]
fn an_ungrouped_component_is_its_own_plan() {
    let plans = plan(UNGROUPED, &ComponentStore::in_memory(), |_| OnDisk::Missing);
    assert_eq!(ids(&plans), vec![vec!["solo"]]);
    assert_eq!(plans[0].group, None);
}

#[test]
fn text_is_written_with_lf_whatever_the_checkout_had() {
    let lf = member("a", Kind::Text("one\ntwo\n"), Root::LocalData, "a", false);
    let crlf = member("b", Kind::Text("one\r\ntwo\r\n"), Root::LocalData, "b", false);
    assert_eq!(crlf.content().unwrap(), b"one\ntwo\n");
    assert_eq!(lf.sha256(), crlf.sha256(), "a checkout's line endings must not change the hash");
}

// --- The real catalog ---------------------------------------------------

fn model() -> &'static Component {
    find(MODEL_ID).expect("the model is in the catalog")
}

fn model_stem() -> &'static str {
    model().path.strip_suffix(".gguf").expect("the model is a .gguf")
}

#[test]
fn catalog_ids_are_unique() {
    let mut seen = std::collections::HashSet::new();
    for c in LLAMA_SERVER {
        assert!(seen.insert(c.id), "duplicate id {}", c.id);
    }
}

#[test]
fn the_model_is_prompted_and_everything_else_is_silent() {
    for c in LLAMA_SERVER {
        assert_eq!(c.prompt, c.id == MODEL_ID, "{}", c.id);
    }
}

/// The router serves a model under its filename stem, and the preset only
/// applies to a section with exactly that name.
#[test]
fn the_preset_has_a_section_for_the_catalog_model() {
    let Kind::Text(ini) = find("k2h-preset").unwrap().kind else { panic!("preset is text") };
    let section = format!("[{}]", model_stem());
    assert!(
        ini.lines().any(|l| l.trim() == section),
        "deploy/llama-models-bearcave.ini has no {section} section"
    );
}

/// A fresh aarch64 config asks for the model the catalog installs.
#[cfg(target_arch = "aarch64")]
#[test]
fn the_default_extract_model_is_the_catalog_model() {
    assert_eq!(crate::llm::ExtractConfig::default().model, model_stem());
}

/// `start-llama-k2horizon.cmd` finds the ini and `llama-server.exe` through
/// `%~dp0`, so the launchers and the ini must land in the runtime dir.
#[test]
fn launchers_and_preset_live_in_the_runtime_dir() {
    let runtime = find("k2h-runtime").unwrap();
    for id in ["k2h-preset", "k2h-launcher", "k2h-launcher-hidden", "k2h-task"] {
        assert!(find(id).unwrap().is_inside(runtime), "{id} must be inside {}", runtime.path);
    }
}

#[test]
fn the_task_launches_a_file_the_catalog_writes() {
    let Kind::Task { launcher, name } = find("k2h-task").unwrap().kind else { panic!("task kind") };
    assert_eq!(name, llama::TASK_NAME);
    assert!(LLAMA_SERVER.iter().any(|c| matches!(c.kind, Kind::Text(_)) && c.path == launcher));
}

#[test]
fn the_cmd_launcher_references_the_embedded_preset_by_name() {
    let Kind::Text(cmd) = find("k2h-launcher").unwrap().kind else { panic!("launcher is text") };
    let preset = find("k2h-preset").unwrap().path.rsplit('/').next().unwrap();
    assert!(cmd.contains(preset), "the .cmd must pass --models-preset {preset}");
}
