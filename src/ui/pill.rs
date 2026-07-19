use dioxus::prelude::*;

/// A small dark glass pill shown at bottom-center of the screen while
/// recording — VibeTyper-style: purple-gradient waveform bars plus an elapsed
/// timer while recording, "Transcribing…" while processing. Rendered in its
/// own transparent, click-through, always-on-top window; state transitions
/// are driven from app.rs via the `beamerSetState` head-script.
#[component]
pub fn RecordingPill() -> Element {
    rsx! {
        div { class: "pill",
            div { class: "pill-bars",
                div { class: "bar bar-1" }
                div { class: "bar bar-2" }
                div { class: "bar bar-3" }
                div { class: "bar bar-4" }
                div { class: "bar bar-5" }
            }
            span { class: "pill-timer", "0:00" }
            span { class: "pill-label", "Transcribing…" }
        }
    }
}

// VibeTyper-style dark glass capsule: purple-gradient waveform, white
// tabular timer, fully rounded, hairline ring, soft shadow.
#[cfg(not(target_os = "linux"))]
pub(super) const PILL_CSS: &str = r#"
*, *::before, *::after { margin:0; padding:0; }
html, body, #main { background:transparent!important; overflow:hidden;
  font-family: "DM Mono","Segoe UI Variable","Segoe UI",monospace,system-ui,sans-serif; }

.pill { display:flex; align-items:center; gap:12px; padding:0 18px;
  height:44px; margin:4px auto; width:fit-content;
  background:linear-gradient(180deg,#17171a,#101012); border-radius:9999px;
  border:1px solid rgba(255,255,255,0.14);
  box-shadow:0 6px 18px rgba(0,0,0,0.45); }

.pill-bars { display:flex; align-items:center; gap:3px; height:26px; }
.bar { width:3px; border-radius:2px;
  animation:wave 1.2s ease-in-out infinite; }
.bar-1{height:8px;  background:#4B0082; animation-delay:0s}
.bar-2{height:16px; background:#6B21A8; animation-delay:.15s}
.bar-3{height:24px; background:#8921E4; animation-delay:.3s}
.bar-4{height:16px; background:#9747F0; animation-delay:.45s}
.bar-5{height:8px;  background:#A561EC; animation-delay:.6s}
@keyframes wave { 0%,100%{transform:scaleY(.4)} 50%{transform:scaleY(1)} }

.pill-bars.processing { gap:5px; }
.pill-bars.processing .bar {
  width:6px; height:6px; border-radius:50%;
  animation:bounce-dot 1.2s ease-in-out infinite; }
.pill-bars.processing .bar-1{animation-delay:0s}
.pill-bars.processing .bar-2{animation-delay:.15s}
.pill-bars.processing .bar-3{animation-delay:.3s}
.pill-bars.processing .bar-4,
.pill-bars.processing .bar-5{display:none}
@keyframes bounce-dot {
  0%,80%,100%{transform:translateY(0)}
  40%{transform:translateY(-8px)} }

.pill-timer { font-size:13px; font-weight:500; color:#ffffff;
  font-variant-numeric:tabular-nums; user-select:none;
  font-family:"DM Mono",monospace; }

.pill-label { font-size:13px; font-weight:500;
  color:rgba(255,255,255,0.85); letter-spacing:.01em; user-select:none;
  font-family:"DM Mono",monospace; display:none; }
"#;

// State transitions for the pill window, injected as a head script so app.rs
// only ever calls `beamerSetState('recording'|'processing'|'idle')`.
#[cfg(not(target_os = "linux"))]
pub(super) const PILL_JS: &str = r#"
window.beamerSetState = function(state) {
  var bars = document.querySelector('.pill-bars');
  var label = document.querySelector('.pill-label');
  var timer = document.querySelector('.pill-timer');
  if (!bars || !label || !timer) return;
  var stopTimer = function() {
    if (window.__beamerTimer) { clearInterval(window.__beamerTimer); window.__beamerTimer = null; }
  };
  if (state === 'recording') {
    bars.className = 'pill-bars';
    label.style.display = 'none';
    timer.style.display = '';
    window.__beamerSecs = 0;
    timer.textContent = '0:00';
    stopTimer();
    window.__beamerTimer = setInterval(function() {
      window.__beamerSecs++;
      var m = Math.floor(window.__beamerSecs / 60);
      var s = ('' + (window.__beamerSecs % 60)).padStart(2, '0');
      var t = document.querySelector('.pill-timer');
      if (t) t.textContent = m + ':' + s;
    }, 1000);
    document.body.style.opacity = '1';
  } else if (state === 'processing') {
    bars.className = 'pill-bars processing';
    label.style.display = '';
    timer.style.display = 'none';
    stopTimer();
    document.body.style.opacity = '1';
  } else {
    stopTimer();
    document.body.style.opacity = '0';
  }
};
"#;
