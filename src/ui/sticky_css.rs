//! Sticky note styling, in Deploy Purple. Split out of `sticky.rs` to keep
//! that file inside the 500-line limit.

/// Sticky note styling, in Deploy Purple. Injected as an inline `<style>` per
/// window, so self-contained (no `@import`); `embedded_font_css()` prepends the
/// faces. Corner radius is the 8px design-system max.
pub const STICKY_CSS: &str = r#"
:root {
  /* Duplicated app tokens — an inline <style> can't reach the main stylesheet. */
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
  /* A pass in flight. Distinct from NoteColor::Amber's #E4A421 dot/fill so a
     running note never reads as "the user colored this note amber". */
  --running:#F97316;
  --duration-fast:150ms;
  --ease:cubic-bezier(0.25,1,0.5,1);
}

*, *::before, *::after { margin:0; padding:0; box-sizing:border-box; }

/* Transparent all the way down, or the webview paints opaque white behind the corners. */
html, body, #main { height:100%; overflow:hidden; background:transparent; }

body { font-family:"Recursive","Segoe UI Variable","Segoe UI",system-ui,sans-serif;
  -webkit-font-smoothing:antialiased; }

/* Room for the hard-offset shadow inside the window, or it clips away. */
#main { padding:0 5px 5px 0; }

.sticky { display:flex; flex-direction:column; height:100%;
  position:relative; /* Anchors the drop-target ring. */
  border:2px solid var(--note-border);
  border-radius:var(--radius-lg);
  overflow:hidden; /* Keeps the bar clipped to the rounded top corners. */
  box-shadow:3px 4px 0 0 var(--note-shadow); }

.sticky-purple { background:#EDE4FB; } .sticky-violet { background:#E4E6FB; }
.sticky-amber  { background:#FBF1DC; } .sticky-teal   { background:#DCF5F0; }
.sticky-rose   { background:#FBE1E8; } .sticky-slate  { background:#E7E9EC; }

/* Title bar: grab cursor + no-select is the drag affordance. */
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

/* Close hover goes danger red. */
.sticky-archive { background:none; border:none; cursor:pointer;
  font-family:"DM Mono","Cascadia Code",monospace;
  font-size:16px; line-height:1; color:var(--ink-soft); padding:2px 5px;
  border-radius:var(--radius-lg);
  transition:color var(--duration-fast) var(--ease),
             background var(--duration-fast) var(--ease); }
.sticky-archive:hover { color:var(--danger); background:rgba(220,38,38,0.10); }

/* --- The block stack -----------------------------------------------------
   One textarea per run + one card per attachment. Textareas size by `rows`
   and never scroll alone, so the note scrolls as one document. */

.sticky-blocks { flex:1; min-height:0; overflow-y:auto;
  display:flex; flex-direction:column; padding:6px 0; }

.sticky-body { width:100%; resize:none; border:none; outline:none;
  overflow:hidden; background:transparent; padding:6px 12px;
  font-family:inherit; font-size:14px;
  line-height:1.5; color:var(--ink); caret-color:var(--accent); }
/* Last run fills the leftover space, so a plain note stays one big textarea. */
.sticky-blocks > .sticky-body:last-child { flex:1 0 auto; }

.sticky-body::selection { background:rgba(75,0,130,0.18); }
.sticky-body::placeholder { color:#94a0b8; }

/* `position:relative` anchors the hover-only remove button. */
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

/* Links/files are one-line chips, not image-sized cards. */
.sticky-link, .sticky-file { display:block; width:100%; text-align:left;
  padding:6px 26px 6px 8px; border:2px solid var(--note-border);
  border-radius:var(--radius); background:rgba(255,255,255,0.5);
  font-family:inherit; font-size:13px; line-height:1.35; color:var(--ink);
  text-decoration:none; cursor:pointer;
  overflow:hidden; text-overflow:ellipsis; white-space:nowrap;
  transition:background var(--duration-fast) var(--ease); }
.sticky-link:hover, .sticky-file:hover { background:rgba(255,255,255,0.9); }

/* Missing file: muted, names the file, offers the fix. */
.sticky-missing { display:flex; flex-direction:column; gap:3px;
  padding:8px; border:2px dashed var(--note-border);
  border-radius:var(--radius); background:rgba(255,255,255,0.35); }
.sticky-missing-title { font-size:11px; letter-spacing:0.04em;
  text-transform:uppercase; color:var(--ink-soft); }
.sticky-missing-name { font-size:13px; color:var(--ink);
  overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
.sticky-locate { align-self:flex-start; cursor:pointer;
  font-size:12px; color:var(--accent); text-decoration:underline; }

/* Orphan token: literal text, so a desync fails visibly. */
.sticky-orphan-token { padding:2px 12px; font-size:13px; line-height:1.5;
  font-family:"DM Mono","Cascadia Code",monospace; color:var(--ink-soft); }

/* Hidden behind a label; `display:none` would make webviews skip the click. */
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

/* Drag feedback: inset ring, so it can't shift layout mid-drag. */
.sticky-drop-target::after { content:""; position:absolute; inset:6px;
  border:2px dashed var(--accent); border-radius:var(--radius);
  pointer-events:none; }

.sticky-gone { display:flex; align-items:center; justify-content:center;
  height:100%; padding:16px; text-align:center;
  font-size:13px; color:var(--ink-soft);
  background:#E7E9EC; border:2px solid var(--note-border);
  border-radius:var(--radius-lg); }

/* --- Model-pass footer + suggestions -------------------------------------
   OUTSIDE .sticky-bar, so no stop_propagation dance; `flex:0 0 auto` keeps
   them from stealing textarea height. */

.sticky-chips { flex:0 0 auto; max-height:44%; overflow-y:auto;
  display:flex; flex-direction:column; gap:6px;
  padding:8px 9px; border-top:2px solid var(--note-border);
  background:var(--note-chrome); }

.sticky-chip { display:flex; align-items:flex-start; gap:8px;
  padding:6px 8px; border:2px solid var(--note-border);
  border-radius:var(--radius); background:rgba(255,255,255,0.5); }

.sticky-chip-main { flex:1; min-width:0; display:flex; flex-direction:column; gap:3px; }
.sticky-chip-text { font-size:13px; line-height:1.35; color:var(--ink); }

/* Evidence span: selection-tinted, so it reads as "this bit of your text". */
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

/* Resize grip: the only resize path on an undecorated window. */
.sticky-grip { margin-left:auto; width:12px; height:12px; flex:0 0 auto;
  cursor:nwse-resize; opacity:0.45;
  background:
    linear-gradient(135deg, transparent 46%, var(--ink-soft) 46%,
                    var(--ink-soft) 54%, transparent 54%),
    linear-gradient(135deg, transparent 76%, var(--ink-soft) 76%,
                    var(--ink-soft) 84%, transparent 84%);
  transition:opacity var(--duration-fast) var(--ease); }
.sticky-grip:hover { opacity:0.9; }

/* Quiet on purpose: the pass already ran; this is the way back, not an ad. */
.sticky-pass { display:flex; align-items:center; justify-content:center;
  width:22px; height:22px; padding:0; border:none; background:none;
  cursor:pointer; color:var(--ink-soft); border-radius:var(--radius);
  opacity:0.55;
  transition:opacity var(--duration-fast) var(--ease),
             color var(--duration-fast) var(--ease),
             background var(--duration-fast) var(--ease); }
.sticky-pass:hover { opacity:1; color:var(--accent); background:rgba(75,0,130,0.08); }
.sticky-pass-done { color:var(--success); }
.sticky-pass-running { color:var(--running); animation:pulse 1.5s ease-in-out infinite; }

@keyframes pulse {
  0%, 100% { opacity:1; }
  50% { opacity:0.4; }
}

/* Footer words (failures only). */
.sticky-pass-error { font-size:11px; color:var(--danger); }
.sticky-paste-hint { font-size:11px; color:var(--ink-soft); }
"#;
