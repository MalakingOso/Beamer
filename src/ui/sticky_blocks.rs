//! The body of a sticky note: a textarea per text run, an inline card per
//! attachment, in body order (`notes::blocks::parse`). The asset handler must
//! register inside `StickyNote`'s own scope (per-window rule — `App()` would
//! bind it to the main window and every image 404s); it serves only paths the
//! note references, and links `prevent_default` + open externally (a bare
//! `href` would strand the webview). Typed/dictated URLs stay plain text.

use dioxus::desktop::wry::http::{Response, StatusCode};
use dioxus::desktop::{use_asset_handler, AssetRequest, RequestAsyncResponder};
use dioxus::prelude::*;
use std::path::{Path, PathBuf};

use crate::notes::blocks::{self, Block};
use crate::notes::{Attachment, Location, NoteStore};

/// First segment of the media URL. Routed by this segment alone, so it must not
/// collide with a real asset directory.
const MEDIA_ROUTE: &str = "note-media";

/// Extensions WebKit can decode; anything else attaches as a file.
const IMAGE_EXTENSIONS: [&str; 8] = ["png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "svg"];

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.as_str()))
}

/// Best-effort MIME type from the extension, for images already chosen to serve.
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

/// Attachment id in a `/note-media/<id>` request path.
pub fn requested_id(path: &str) -> Option<&str> {
    let id = path.trim_start_matches('/').strip_prefix(MEDIA_ROUTE)?.trim_start_matches('/');
    (!id.is_empty()).then_some(id)
}

/// Serve this note's images to its own webview. Call from inside `StickyNote`:
/// registration binds to the `DesktopContext` in scope and dies with the window.
pub fn use_note_media(note_id: String, notes: Signal<NoteStore>) {
    use_asset_handler(MEDIA_ROUTE, move |request: AssetRequest, responder: RequestAsyncResponder| {
        let path = request.uri().path().to_string();
        let if_none_match = request
            .headers()
            .get("if-none-match")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        let resolved = requested_id(&path).and_then(|id| {
            // Resolved through the note's own attachments — no path in the URL,
            // so no request can name an unreferenced file.
            let store = notes.peek();
            let note = store.get(&note_id)?;
            let attachment = NoteStore::attachment(note, id)?;
            let file = attachment.resolved_path(&store.attachments_dir)?;
            // Only `Owned` is content-addressed (filename is the hash), so only
            // it gets an ETag; `External` falls through to `no-store` below.
            let etag = match attachment.location() {
                Some(Location::Owned { hash, .. }) => Some(format!("\"{hash}\"")),
                _ => None,
            };
            Some((file, etag))
        });

        let Some((file, etag)) = resolved else {
            return responder.respond(not_found("no such attachment on this note"));
        };

        if let (Some(etag), Some(if_none_match)) = (&etag, &if_none_match) {
            if etag == if_none_match {
                tracing::debug!("note media {:?} matched If-None-Match, sending 304", file);
                return responder.respond(
                    Response::builder()
                        .status(StatusCode::NOT_MODIFIED)
                        .header("ETag", etag.as_str())
                        .body(Vec::new())
                        .unwrap_or_else(|_| not_found("could not build a response")),
                );
            }
        }

        match std::fs::read(&file) {
            Ok(bytes) => {
                let builder = Response::builder().header("Content-Type", content_type(&file));
                let builder = match &etag {
                    // `no-cache`, not "never cache": the URL is keyed on attachment
                    // id, and "Locate…" can repoint an id at a new hash under it.
                    Some(etag) => builder.header("ETag", etag.as_str()).header("Cache-Control", "no-cache"),
                    None => builder.header("Cache-Control", "no-store"),
                };
                responder.respond(
                    builder
                        .body(bytes)
                        .unwrap_or_else(|_| not_found("could not build a response")),
                )
            }
            // Quiet: the block renders a missing-file card for the same fact.
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

/// Rows for a run's textarea, from line count (not measured JS — a
/// `document::eval` roundtrip per keystroke would sit behind the IPC bridge).
pub fn rows_for(text: &str, is_only_run: bool) -> usize {
    let counted = text.lines().count().max(1);
    // A lone run fills the note rather than hugging two lines of dictation.
    let floor = if is_only_run { 3 } else { 1 };
    counted.max(floor).min(40)
}

#[derive(Props, Clone, PartialEq)]
pub struct StickyBodyProps {
    pub id: String,
    pub notes: Signal<NoteStore>,
    /// Passed down so this re-renders with the parent, not via a second store subscription.
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
                                    // Re-read: a model pass may have rewritten the
                                    // body since render; editing a stale copy would undo it.
                                    let current = notes.peek().get(&id).map(|n| n.body.clone());
                                    let Some(current) = current else { return };
                                    let next = blocks::set_run(&current, index, &e.value());
                                    let mut store = notes.write();
                                    store.set_body(&id, next);
                                    // Same write, so a deleted token and its
                                    // attachment record can't go out of step on disk.
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
                            // Token with no record: render literally so a
                            // desynchronised note fails visibly, not silently.
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
    // Checked separately: an `Owned` file can be missing too (unsynced, hand-removed).
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
                // Missing file: name, full path on hover, and a Locate fix.
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
                        // Without this the webview navigates away and never comes back.
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

/// Repoint a moved file via a hidden file input (native dialog, same as the paperclip).
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

/// Turn a dropped path into an `External` attachment. Inspects the path only —
/// `NoteStore::add_attachment` does the actual copy, so this touches no filesystem.
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

/// First bare `http(s)` URL in dropped/pasted text. Anything else (prose
/// mentioning a site, `ftp:`, …) is not a link — it must insert text, not a chip.
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
        // Lookup is by exact id equality in the note's own list, so this matches nothing.
        assert_eq!(requested_id("/note-media/../../etc/passwd"), Some("../../etc/passwd"));
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
        assert_eq!(url_in("see https://example.com for details"), None);
        assert_eq!(url_in("just some text"), None);
        assert_eq!(url_in("ftp://files.example.com"), None, "only http(s) is opened");
        assert_eq!(url_in(""), None);
    }
}
