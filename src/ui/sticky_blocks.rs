//! The body of a sticky note, as a stack of blocks.
//!
//! A note used to be one `<textarea>` bound to one `String`. It is now a stack
//! driven by `notes::blocks::parse`: a textarea per text run, an inline card per
//! attachment, in the order the body puts them.
//!
//! ```text
//! ┌─ ●●●●●● ──────────── 📎 × ─┐
//! │ Ring Sarah about the       │  <- textarea, run 0
//! │ ┌────────────────────────┐ │
//! │ │      [image]        ⤫  │ │  <- attachment block
//! │ └────────────────────────┘ │
//! │ Q3 deck before Friday.     │  <- textarea, run 1
//! │ 🔗 figma.com/file/…     ⤫  │  <- attachment block
//! └────────────────────────────┘
//! ```
//!
//! Three things here are load-bearing and easy to get wrong:
//!
//! - **The asset handler is registered inside `StickyNote`'s own scope**, from
//!   `use_note_media`. `use_asset_handler` resolves `crate::window()` by
//!   `consume_context`, so registering it in `App()` would bind it to the main
//!   window and every note's image would 404 — silently, as a broken image.
//!   Same per-window rule that makes `use_wry_event_handler` work in `sticky.rs`
//!   and nowhere else.
//! - **It serves only paths the note itself references.** The id is resolved
//!   against that note's `attachments`; there is no path in the URL and no way
//!   to ask for one.
//! - **Links never navigate.** A bare `href` would take the webview away from
//!   `dioxus://index.html` and the note would never come back. The handler
//!   calls `prevent_default` and opens the system browser instead.
//!
//! ⚠️ Only **dropped or pasted** links become chips. A URL typed or dictated
//! inside a text run stays plain text — linkifying inside a `<textarea>` is not
//! possible, and pretending otherwise would mean replacing the editor.

use dioxus::desktop::wry::http::{Response, StatusCode};
use dioxus::desktop::{use_asset_handler, AssetRequest, RequestAsyncResponder};
use dioxus::prelude::*;
use std::path::{Path, PathBuf};

use crate::notes::blocks::{self, Block};
use crate::notes::{Attachment, Location, NoteStore};

/// First path segment of the URL images are served from. Routing is by this
/// segment alone (`dioxus-desktop/src/protocol.rs`), so it must not collide
/// with a real asset directory.
const MEDIA_ROUTE: &str = "note-media";

/// Extensions WebKit can decode as an image. Anything else attaches as a file
/// rather than rendering as a broken picture.
const IMAGE_EXTENSIONS: [&str; 8] = ["png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "svg"];

/// Whether a dropped file should render inline.
pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.as_str()))
}

/// Best-effort MIME type, from the extension. Only ever used for images we
/// already decided to serve.
fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("bmp") => "image/bmp",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// The attachment id in a `/note-media/<id>` request path.
pub fn requested_id(path: &str) -> Option<&str> {
    let id = path.trim_start_matches('/').strip_prefix(MEDIA_ROUTE)?.trim_start_matches('/');
    (!id.is_empty()).then_some(id)
}

/// Serve this note's images to its own webview.
///
/// **Call from inside `StickyNote`.** See the module note: the registration
/// binds to whichever `DesktopContext` is in scope, and it is torn down with
/// the window, so no cleanup is needed here.
pub fn use_note_media(note_id: String, notes: Signal<NoteStore>) {
    use_asset_handler(MEDIA_ROUTE, move |request: AssetRequest, responder: RequestAsyncResponder| {
        let path = request.uri().path().to_string();
        let file = requested_id(&path).and_then(|id| {
            // Resolved through the note's own attachment list. There is no path
            // in the URL, so no request can name a file this note does not
            // already reference.
            let store = notes.peek();
            let note = store.get(&note_id)?;
            NoteStore::attachment(note, id)?.resolved_path(&store.attachments_dir)
        });

        let Some(file) = file else {
            return responder.respond(not_found("no such attachment on this note"));
        };
        match std::fs::read(&file) {
            Ok(bytes) => responder.respond(
                Response::builder()
                    .header("Content-Type", content_type(&file))
                    // The file can be replaced under us by "Locate…", and the
                    // URL does not change when it is.
                    .header("Cache-Control", "no-store")
                    .body(bytes)
                    .unwrap_or_else(|_| not_found("could not build a response")),
            ),
            // The whole failure mode reference-by-path buys. Not an error worth
            // logging loudly: the block renders a missing-file card, which is
            // the user-facing half of the same fact.
            Err(e) => {
                tracing::debug!("note media {:?} is unreadable: {}", file, e);
                responder.respond(not_found("file is gone"))
            }
        }
    });
}

fn not_found(why: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(why.as_bytes().to_vec())
        .expect("a static 404 body always builds")
}

/// How tall to make a run's textarea, in rows.
///
/// Computed from the text rather than measured in JS: a `document::eval`
/// roundtrip per keystroke to read `scrollHeight` would put the editor's
/// responsiveness behind the IPC bridge for a number that line counting gets
/// right. Long lines wrap and are undercounted, which is why there is a floor
/// and the CSS lets the stack scroll.
pub fn rows_for(text: &str, is_only_run: bool) -> usize {
    let counted = text.lines().count().max(1);
    // A lone run owns the whole note, so it should fill it rather than hug two
    // lines of dictation.
    let floor = if is_only_run { 3 } else { 1 };
    counted.max(floor).min(40)
}

#[derive(Props, Clone, PartialEq)]
pub struct StickyBodyProps {
    pub id: String,
    pub notes: Signal<NoteStore>,
    /// The note's body, passed down so this component re-renders with the
    /// parent rather than subscribing to the store a second time.
    pub body: String,
    pub attachments: Vec<Attachment>,
}

#[component]
pub fn StickyBody(props: StickyBodyProps) -> Element {
    let StickyBodyProps { id, mut notes, body, attachments } = props;

    let blocks = blocks::parse(&body);
    let total_runs = blocks.iter().filter(|b| matches!(b, Block::Text(_))).count();
    let mut run_index = 0usize;

    rsx! {
        div { class: "sticky-blocks",
            for (i, block) in blocks.iter().enumerate() {
                match block {
                    Block::Text(text) => {
                        let index = run_index;
                        run_index += 1;
                        let id = id.clone();
                        rsx! {
                            textarea {
                                key: "run-{i}",
                                class: "sticky-body",
                                spellcheck: false,
                                rows: "{rows_for(text, total_runs == 1)}",
                                value: "{text}",
                                oninput: move |e| {
                                    // The body is re-read here rather than
                                    // captured at render: a model pass can
                                    // rewrite it between the two, and editing a
                                    // stale copy would undo the rewrite.
                                    let current = notes.peek().get(&id).map(|n| n.body.clone());
                                    let Some(current) = current else { return };
                                    let next = blocks::set_run(&current, index, &e.value());
                                    let mut store = notes.write();
                                    store.set_body(&id, next);
                                    // Deleting a token out of the textarea is
                                    // the one gesture that orphans an
                                    // attachment record. Collected in the same
                                    // write, so the record and the token can
                                    // never be out of step on disk.
                                    store.prune_attachments(&id);
                                },
                            }
                        }
                    }
                    Block::Attachment(att_id) => {
                        let found = attachments.iter().find(|a| a.id() == *att_id).cloned();
                        match found {
                            Some(attachment) => rsx! {
                                AttachmentBlock {
                                    key: "att-{i}",
                                    note_id: id.clone(),
                                    notes,
                                    attachment,
                                }
                            },
                            // A token with no record. Rendered as the literal
                            // text it is, so a desynchronised note is visibly
                            // wrong rather than a line that silently vanished.
                            None => rsx! {
                                div { key: "orphan-{i}", class: "sticky-orphan-token",
                                    "[[beamer:{att_id}]]"
                                }
                            },
                        }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct AttachmentBlockProps {
    note_id: String,
    notes: Signal<NoteStore>,
    attachment: Attachment,
}

#[component]
fn AttachmentBlock(props: AttachmentBlockProps) -> Element {
    let AttachmentBlockProps { note_id, mut notes, attachment } = props;

    let att_id = attachment.id().to_string();
    let label = attachment.label();
    let attachments_dir = notes.peek().attachments_dir.clone();
    let resolved = attachment.resolved_path(&attachments_dir);
    // Existence is checked separately from what `resolved_path` returns: an
    // `Owned` file can still be missing (not synced yet, removed by hand
    // outside Beamer) just as an `External` one always could be.
    let missing = resolved.as_deref().is_some_and(|p| !p.exists());

    let remove = {
        let note_id = note_id.clone();
        let att_id = att_id.clone();
        move |e: Event<MouseData>| {
            e.stop_propagation();
            notes.write().remove_attachment(&note_id, &att_id);
        }
    };

    rsx! {
        div { class: "sticky-attachment",
            match (&attachment, missing) {
                // The failure mode reference-by-path buys, made visible: the
                // name, the full path on hover, and a way to fix it. Never a
                // broken-image icon and never a blank gap.
                (_, true) => rsx! {
                    div { class: "sticky-missing",
                        span { class: "sticky-missing-title", "Missing file" }
                        span {
                            class: "sticky-missing-name",
                            title: "{resolved.as_ref().map(|p| p.display().to_string()).unwrap_or_default()}",
                            "{label}"
                        }
                        Locate { note_id: note_id.clone(), notes, attachment_id: att_id.clone() }
                    }
                },
                (Attachment::Image { .. }, false) => rsx! {
                    img {
                        class: "sticky-image",
                        src: "/{MEDIA_ROUTE}/{att_id}",
                        alt: "{label}",
                        title: "{label}",
                    }
                },
                (Attachment::Link { url, .. }, false) => rsx! {
                    a {
                        class: "sticky-link",
                        href: "{url}",
                        title: "{url}",
                        // Without this the webview navigates away from
                        // dioxus://index.html and the note never comes back.
                        onclick: {
                            let url = url.clone();
                            move |e: Event<MouseData>| {
                                e.prevent_default();
                                crate::ui::open_external(&url);
                            }
                        },
                        "\u{1F517} {label}"
                    }
                },
                (Attachment::File { .. }, false) => rsx! {
                    button {
                        class: "sticky-file",
                        title: "{label}",
                        onclick: {
                            // `resolved` is `Some` here: `missing` above would
                            // have been true otherwise, and this arm only
                            // matches `(_, false)`.
                            let path = resolved.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
                            move |_| crate::ui::open_external(&path)
                        },
                        "\u{1F4CE} {label}"
                    }
                },
            }
            button {
                class: "sticky-attachment-remove",
                title: "Remove this attachment",
                onmousedown: move |e: Event<MouseData>| e.stop_propagation(),
                onclick: remove,
                "\u{2A2F}"
            }
        }
    }
}

/// Repoint a moved file. A hidden `<input type="file">` behind a label, which
/// dioxus-desktop turns into a native dialog returning a real path — no new
/// dependency, and the same mechanism as the paperclip button.
#[derive(Props, Clone, PartialEq)]
struct LocateProps {
    note_id: String,
    notes: Signal<NoteStore>,
    attachment_id: String,
}

#[component]
fn Locate(props: LocateProps) -> Element {
    let LocateProps { note_id, mut notes, attachment_id } = props;
    rsx! {
        label { class: "sticky-locate",
            "Locate\u{2026}"
            input {
                r#type: "file",
                class: "sticky-file-input",
                onchange: move |e: Event<FormData>| {
                    let Some(file) = e.files().into_iter().next() else { return };
                    notes.write().relocate_attachment(&note_id, &attachment_id, file.path());
                },
            }
        }
    }
}

/// Turn a dropped path into the attachment it should become.
///
/// Built as `External`, pointing at wherever the drop or file picker says the
/// file is right now. `NoteStore::add_attachment` is what actually tries to
/// copy the bytes in; this function only ever inspects the path, never reads
/// it, which is why it stays free of filesystem access.
pub fn attachment_for_path(id: String, path: PathBuf) -> Attachment {
    let filename = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let image = is_image(&path);
    let location = Location::External { path };
    if image {
        Attachment::Image { id, filename, alt: None, location }
    } else {
        Attachment::File { id, filename, location }
    }
}

/// The first `http(s)` URL in some dropped or pasted text.
///
/// `text/uri-list` may carry several lines and `#`-prefixed comments; plain
/// text may be a URL with whitespace around it and nothing else. Anything that
/// is not a bare URL is **not** a link — pasting a paragraph that happens to
/// mention a site must insert text, not a chip.
pub fn url_in(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .find(|l| {
            (l.starts_with("http://") || l.starts_with("https://"))
                && !l.contains(char::is_whitespace)
        })
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_webkit_decodable_rasters_render_inline() {
        for name in ["a.png", "b.JPG", "c.jpeg", "d.gif", "e.webp", "f.avif", "g.bmp", "h.svg"] {
            assert!(is_image(Path::new(name)), "{name} should render inline");
        }
        for name in ["notes.pdf", "deck.key", "archive.tar.gz", "no-extension"] {
            assert!(
                !is_image(Path::new(name)),
                "{name} would render as a broken image, so it attaches as a file"
            );
        }
    }

    #[test]
    fn the_media_route_reads_the_id_and_nothing_else() {
        assert_eq!(requested_id("/note-media/a1"), Some("a1"));
        assert_eq!(requested_id("note-media/a1"), Some("a1"));
        assert_eq!(
            requested_id("/note-media/"),
            None,
            "an empty id must not resolve to anything"
        );
        assert_eq!(requested_id("/other/a1"), None);
    }

    #[test]
    fn a_media_path_carries_no_filesystem_path_to_traverse() {
        // The URL names an attachment id, never a file. Whatever the id says,
        // it is only ever looked up in the note's own attachment list.
        assert_eq!(requested_id("/note-media/../../etc/passwd"), Some("../../etc/passwd"));
        // ...and that lookup is by exact id equality, so the string above
        // simply matches nothing.
        let store = NoteStore::default();
        assert!(store.get("../../etc/passwd").is_none());
    }

    #[test]
    fn content_type_falls_back_rather_than_guessing() {
        assert_eq!(content_type(Path::new("a.png")), "image/png");
        assert_eq!(content_type(Path::new("a.JPEG")), "image/jpeg");
        assert_eq!(content_type(Path::new("a.weird")), "application/octet-stream");
    }

    #[test]
    fn a_lone_run_gets_room_and_a_run_beside_an_image_does_not() {
        assert_eq!(rows_for("", true), 3, "an empty note should not be a one-line slot");
        assert_eq!(rows_for("one", false), 1);
        assert_eq!(rows_for("a\nb\nc", false), 3);
        assert_eq!(
            rows_for(&"x\n".repeat(200), false),
            40,
            "a runaway note must not make a window of unbounded height"
        );
    }

    #[test]
    fn a_dropped_image_renders_inline_and_anything_else_attaches() {
        let img = attachment_for_path("a1".into(), PathBuf::from("/pics/cat.png"));
        assert!(matches!(img, Attachment::Image { .. }));
        let doc = attachment_for_path("a2".into(), PathBuf::from("/docs/spec.pdf"));
        assert!(matches!(doc, Attachment::File { .. }));
    }

    #[test]
    fn a_uri_list_yields_its_first_real_url() {
        assert_eq!(
            url_in("# comment\nhttps://figma.com/file/abc\nhttps://second"),
            Some("https://figma.com/file/abc".to_string())
        );
        assert_eq!(url_in("  http://example.com  "), Some("http://example.com".to_string()));
    }

    #[test]
    fn prose_that_merely_mentions_a_site_is_not_a_link() {
        // Pasting a paragraph must insert text. A chip would silently swallow
        // the rest of what was on the clipboard.
        assert_eq!(url_in("see https://example.com for details"), None);
        assert_eq!(url_in("just some text"), None);
        assert_eq!(url_in("ftp://files.example.com"), None, "only http(s) is opened");
        assert_eq!(url_in(""), None);
    }
}
