//! Sticky note styling, in Deploy Purple.
//!
//! Split out of `sticky.rs` so that file has room for behaviour. The note
//! component grows a suggestion strip and a footer; the stylesheet is the half
//! nobody needs to read while working on either, and leaving the two together
//! would have spent the 500-line limit on CSS.

/// Sticky note styling, in Deploy Purple.
///
/// Injected as an inline `<style>` per window, so it must be self-contained —
/// it cannot `@import` the app stylesheet. `embedded_font_css()` is prepended
/// at window creation to supply the faces this references.
///
/// Corner radius is 8px because `agent_docs/design_system.md` caps it there
/// ("Sharp system: 4px base, 6px cards, 8px max. Never rounder."). Notes take
/// the maximum, which makes them the softest surface in the app without
/// leaving its language.
pub const STICKY_CSS: &str = r#"
:root {
  /* Mirrors the app tokens in assets/styles.css. Duplicated, not imported:
     an inline <style> has no access to the main stylesheet. */
  --ink:#0f152a;
  --ink-soft:#64708b;
  --accent:#4B0082;
  --danger:#DC2626;
  --note-border:rgba(75,0,130,0.28);
  --note-shadow:rgba(75,0,130,0.18);
  --note-chrome:rgba(255,255,255,0.42);
  --radius:4px;
  --radius-lg:8px;
  --success:#16A34A;
  --duration-fast:150ms;
  --ease:cubic-bezier(0.25,1,0.5,1);
}

*, *::before, *::after { margin:0; padding:0; box-sizing:border-box; }

/* Transparent all the way down. The window is built with_transparent(true)
   and a (0,0,0,0) background color; without these the webview still paints an
   opaque white sheet and the rounded corners show it as square white nubs. */
html, body, #main { height:100%; overflow:hidden; background:transparent; }

body { font-family:"Recursive","Segoe UI Variable","Segoe UI",system-ui,sans-serif;
  -webkit-font-smoothing:antialiased; }

/* Room for the hard-offset shadow to render inside the window. Without this
   the shadow is clipped by the window edge and simply never appears. */
#main { padding:0 5px 5px 0; }

.sticky { display:flex; flex-direction:column; height:100%;
  /* Anchors the drop-target ring below. */
  position:relative;
  border:2px solid var(--note-border);
  border-radius:var(--radius-lg);
  /* Clips the bar's fill and border-bottom to the rounded top corners —
     without it the bar squares them off again. */
  overflow:hidden;
  box-shadow:3px 4px 0 0 var(--note-shadow); }

.sticky-purple { background:#EDE4FB; } .sticky-violet { background:#E4E6FB; }
.sticky-amber  { background:#FBF1DC; } .sticky-teal   { background:#DCF5F0; }
.sticky-rose   { background:#FBE1E8; } .sticky-slate  { background:#E7E9EC; }

/* The title bar. `cursor:grab` and `user-select:none` are the affordance for
   the window drag wired up in StickyNote — a bar that selects text on
   press-and-move reads as broken even when the drag works. */
.sticky-bar { display:flex; align-items:center; justify-content:space-between;
  padding:7px 9px; border-bottom:2px solid var(--note-border);
  background:var(--note-chrome);
  cursor:grab; user-select:none; -webkit-user-select:none; }
.sticky-bar:active { cursor:grabbing; }

.sticky-dots { display:flex; gap:6px; }
.sticky-dot { width:12px; height:12px; border:1.5px solid var(--note-border);
  border-radius:50%; cursor:pointer; padding:0;
  transition:transform var(--duration-fast) var(--ease); }
.sticky-dot:hover { transform:scale(1.18); }
.sticky-dot-purple{background:#8921E4} .sticky-dot-violet{background:#6B6BE4}
.sticky-dot-amber {background:#E4A421} .sticky-dot-teal  {background:#21C9B0}
.sticky-dot-rose  {background:#E4216B} .sticky-dot-slate {background:#8A93A0}

/* Close hover goes danger red, per the custom-title-bar spec. */
.sticky-archive { background:none; border:none; cursor:pointer;
  font-family:"DM Mono","Cascadia Code",monospace;
  font-size:16px; line-height:1; color:var(--ink-soft); padding:2px 5px;
  border-radius:var(--radius-lg);
  transition:color var(--duration-fast) var(--ease),
             background var(--duration-fast) var(--ease); }
.sticky-archive:hover { color:var(--danger); background:rgba(220,38,38,0.10); }

/* --- The block stack -----------------------------------------------------
   One textarea per text run, one card per attachment, in the order the body
   puts them. The stack scrolls; each textarea is sized by its `rows` attribute
   (computed in Rust from the line count) and does not scroll on its own, so a
   run always shows all of itself and the note scrolls as one document. */

.sticky-blocks { flex:1; min-height:0; overflow-y:auto;
  display:flex; flex-direction:column; padding:6px 0; }

.sticky-body { width:100%; resize:none; border:none; outline:none;
  overflow:hidden; background:transparent; padding:6px 12px;
  font-family:inherit; font-size:14px;
  line-height:1.5; color:var(--ink); caret-color:var(--accent); }
/* The last run fills whatever is left, so a plain note is one big clickable
   textarea exactly as it was before the stack existed. Without this a
   two-line note leaves ~100px of dead space that focuses nothing — and a note
   with no attachments is the common case. `parse` always ends with a text run,
   so this selector always matches. */
.sticky-blocks > .sticky-body:last-child { flex:1 0 auto; }

.sticky-body::selection { background:rgba(75,0,130,0.18); }
.sticky-body::placeholder { color:#94a0b8; }

/* An attachment. `position:relative` anchors the remove button, which only
   appears on hover — an always-visible ⤫ on every image would make a note of
   three photos read as a toolbar. */
.sticky-attachment { position:relative; margin:2px 12px 6px; }
.sticky-attachment-remove { position:absolute; top:4px; right:4px;
  display:flex; align-items:center; justify-content:center;
  width:20px; height:20px; padding:0; border:none; cursor:pointer;
  border-radius:var(--radius); background:rgba(255,255,255,0.86);
  color:var(--ink-soft); font-size:13px; line-height:1;
  opacity:0; transition:opacity var(--duration-fast) var(--ease),
                        color var(--duration-fast) var(--ease); }
.sticky-attachment:hover .sticky-attachment-remove { opacity:1; }
.sticky-attachment-remove:hover { color:var(--danger); }

.sticky-image { display:block; width:100%; height:auto; max-height:420px;
  object-fit:contain; border:2px solid var(--note-border);
  border-radius:var(--radius); background:rgba(255,255,255,0.5); }

/* Links and files are chips, not blocks: they are one line of text and giving
   them an image-sized card would waste the note. */
.sticky-link, .sticky-file { display:block; width:100%; text-align:left;
  padding:6px 26px 6px 8px; border:2px solid var(--note-border);
  border-radius:var(--radius); background:rgba(255,255,255,0.5);
  font-family:inherit; font-size:13px; line-height:1.35; color:var(--ink);
  text-decoration:none; cursor:pointer;
  overflow:hidden; text-overflow:ellipsis; white-space:nowrap;
  transition:background var(--duration-fast) var(--ease); }
.sticky-link:hover, .sticky-file:hover { background:rgba(255,255,255,0.9); }

/* The failure mode reference-by-path buys, made visible. Muted rather than
   alarming: the file moved, which is a thing that happens, and the card says
   which file and offers the fix. */
.sticky-missing { display:flex; flex-direction:column; gap:3px;
  padding:8px; border:2px dashed var(--note-border);
  border-radius:var(--radius); background:rgba(255,255,255,0.35); }
.sticky-missing-title { font-size:11px; letter-spacing:0.04em;
  text-transform:uppercase; color:var(--ink-soft); }
.sticky-missing-name { font-size:13px; color:var(--ink);
  overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
.sticky-locate { align-self:flex-start; cursor:pointer;
  font-size:12px; color:var(--accent); text-decoration:underline; }

/* A token whose attachment record is gone. Rendered as the literal text it is
   — a desynchronised note must fail visibly, not swallow a line. */
.sticky-orphan-token { padding:2px 12px; font-size:13px; line-height:1.5;
  font-family:"DM Mono","Cascadia Code",monospace; color:var(--ink-soft); }

/* Native file inputs cannot be styled, so every one of them is hidden behind
   a label. `display:none` would make some webviews skip the click entirely. */
.sticky-file-input { position:absolute; width:0; height:0; opacity:0;
  pointer-events:none; }

.sticky-bar-actions { display:flex; align-items:center; gap:2px; }
.sticky-new, .sticky-attach { display:flex; align-items:center; justify-content:center;
  cursor:pointer; font-size:13px; line-height:1; padding:3px 5px;
  border-radius:var(--radius-lg); opacity:0.6;
  transition:opacity var(--duration-fast) var(--ease),
              background var(--duration-fast) var(--ease); }
.sticky-new { border:none; background:none; color:var(--ink-soft); }
.sticky-new:hover, .sticky-attach:hover {
  opacity:1; background:rgba(75,0,130,0.08); }

/* Drag feedback. An inset dashed accent rather than an outline, so it cannot
   push the layout around mid-drag. */
.sticky-drop-target::after { content:""; position:absolute; inset:6px;
  border:2px dashed var(--accent); border-radius:var(--radius);
  pointer-events:none; }

.sticky-gone { display:flex; align-items:center; justify-content:center;
  height:100%; padding:16px; text-align:center;
  font-size:13px; color:var(--ink-soft);
  background:#E7E9EC; border:2px solid var(--note-border);
  border-radius:var(--radius-lg); }

/* --- The model-pass footer and its suggestions ---------------------------
   Both sit OUTSIDE .sticky-bar, as further flex children of .sticky. Anything
   inside the bar has to call stop_propagation on mousedown or the window drag
   eats the click; out here that dance is unnecessary. `flex:0 0 auto` keeps
   them from stealing height from the textarea. */

.sticky-chips { flex:0 0 auto; max-height:44%; overflow-y:auto;
  display:flex; flex-direction:column; gap:6px;
  padding:8px 9px; border-top:2px solid var(--note-border);
  background:var(--note-chrome); }

.sticky-chip { display:flex; align-items:flex-start; gap:8px;
  padding:6px 8px; border:2px solid var(--note-border);
  border-radius:var(--radius); background:rgba(255,255,255,0.5); }

.sticky-chip-main { flex:1; min-width:0; display:flex; flex-direction:column; gap:3px; }
.sticky-chip-text { font-size:13px; line-height:1.35; color:var(--ink); }

/* The span of the note that produced the task. Tinted the same way a
   selection is, so it reads as "this bit of your text" on all six note
   colours rather than as a second paragraph. */
.sticky-chip-evidence { font-size:11px; line-height:1.35; color:var(--ink-soft);
  background:rgba(75,0,130,0.10); border-radius:var(--radius);
  padding:2px 5px; align-self:flex-start;
  overflow:hidden; text-overflow:ellipsis; white-space:nowrap; max-width:100%; }

.sticky-chip-actions { display:flex; gap:2px; flex:0 0 auto; }
.sticky-chip-btn { display:flex; align-items:center; justify-content:center;
  width:24px; height:24px; padding:0; border:none; background:none;
  cursor:pointer; color:var(--ink-soft); border-radius:var(--radius);
  transition:color var(--duration-fast) var(--ease),
             background var(--duration-fast) var(--ease); }
.sticky-chip-accept:hover { color:var(--success); background:rgba(22,163,74,0.12); }
.sticky-chip-dismiss:hover { color:var(--danger); background:rgba(220,38,38,0.10); }

.sticky-footer { flex:0 0 auto; display:flex; align-items:center; gap:8px;
  padding:4px 9px 6px; }

/* The resize grip. `with_decorations(false)` means the compositor offers no
   edge to grab, so this is the only way to resize a note. Drawn as two corner
   rules rather than an icon: it has to read as a grip at 12px. */
.sticky-grip { margin-left:auto; width:12px; height:12px; flex:0 0 auto;
  cursor:nwse-resize; opacity:0.45;
  background:
    linear-gradient(135deg, transparent 46%, var(--ink-soft) 46%,
                    var(--ink-soft) 54%, transparent 54%),
    linear-gradient(135deg, transparent 76%, var(--ink-soft) 76%,
                    var(--ink-soft) 84%, transparent 84%);
  transition:opacity var(--duration-fast) var(--ease); }
.sticky-grip:hover { opacity:0.9; }

/* Quiet on purpose. The cleanup pass has already run by the time the note is
   on screen; this is the way back to it, not an invitation to invoke a model. */
.sticky-pass { display:flex; align-items:center; justify-content:center;
  width:22px; height:22px; padding:0; border:none; background:none;
  cursor:pointer; color:var(--ink-soft); border-radius:var(--radius);
  opacity:0.55;
  transition:opacity var(--duration-fast) var(--ease),
             color var(--duration-fast) var(--ease),
             background var(--duration-fast) var(--ease); }
.sticky-pass:hover { opacity:1; color:var(--accent); background:rgba(75,0,130,0.08); }
.sticky-pass-done { color:var(--success); }

/* The two places the footer uses words. */
.sticky-pass-error { font-size:11px; color:var(--danger); }
.sticky-paste-hint { font-size:11px; color:var(--ink-soft); }
"#;
