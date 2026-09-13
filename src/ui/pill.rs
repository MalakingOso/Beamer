use dioxus::prelude::*;

use crate::hotkey::CaptureMode;
use crate::orchestrator::RecordingState;

/// Pill style for a recording state + capture mode, or `None` to hide. Shared by
/// both platforms' pills (GNOME `indicator.js` and `beamerSetState` branch on
/// these exact strings — a typo reads as a pill ignoring the mic), so one
/// function keeps them from drifting. Factored out for testing without a runtime.
pub(super) fn pill_state(state: RecordingState, mode: CaptureMode) -> Option<&'static str> {
    match (state, mode) {
        (RecordingState::Idle, _) => None,
        (RecordingState::Recording, CaptureMode::Note) => Some("note"),
        (RecordingState::Recording, CaptureMode::Inject) => Some("recording"),
        (RecordingState::Processing, _) => Some("processing"),
    }
}

/// Beamer Purple recording pill (bottom-center): 12-bar waveform following mic
/// level, plus "Transcribing…" while processing; no elapsed-time readout (mirrors
/// the GNOME pill). Own transparent, click-through, always-on-top window, driven
/// via the `beamerSetState`/`beamerSetLevel` head-script.
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
                div { class: "bar bar-6" }
                div { class: "bar bar-7" }
                div { class: "bar bar-8" }
                div { class: "bar bar-9" }
                div { class: "bar bar-10" }
                div { class: "bar bar-11" }
                div { class: "bar bar-12" }
            }
            span { class: "pill-label", "Transcribing…" }
        }
    }
}

/// Pill window size (logical px) and where its ink stops. The window is a
/// viewport with `overflow:hidden`: anything drawn past its edge is silently cut
/// (a too-short window once flattened the pill's bottom edge with no error).
/// Height = 56px border box + 4px shadow + 20px entrance translate; width fits
/// the widest state (`processing` + label). Keep the three numbers in step.
#[cfg(not(target_os = "linux"))]
pub(super) const PILL_WINDOW_W: f64 = 264.0;
#[cfg(not(target_os = "linux"))]
pub(super) const PILL_WINDOW_H: f64 = 80.0;
#[cfg(not(target_os = "linux"))]
pub(super) const PILL_INK_BOTTOM: f64 = 60.0;

// Beamer Purple card matching the GNOME pill's tokens, shape, and waveform
// gradient (bar colors are indicator.js's COLOR_FROM→COLOR_TO lerp, `i/11`).
#[cfg(not(target_os = "linux"))]
pub(super) const PILL_CSS: &str = r#"
*, *::before, *::after { margin:0; padding:0; }
html, body, #main { background:transparent!important; overflow:hidden;
  font-family: "DM Mono","Segoe UI Variable","Segoe UI",monospace,system-ui,sans-serif; }

.pill { display:flex; align-items:center; gap:12px; padding:0 18px;
  height:48px; margin:4px auto; width:fit-content;
  background:#fbfbfd; border-radius:8px;
  border:2px solid rgba(75,0,130,0.25);
  box-shadow:2px 4px 0 0 rgba(75,0,130,0.15);
  /* Bottom-anchored like indicator.js's pivot: scale grows from the bottom edge. */
  transform-origin:50% 100%;
  opacity:0; transform:translateY(20px) scale(0.92); }

.pill-note { border:2px solid rgba(137,33,228,0.85);
  box-shadow:2px 4px 0 0 rgba(137,33,228,0.25); }

.pill-bars { display:flex; align-items:center; gap:3px; height:26px; }
.bar { width:3px; height:4px; border-radius:2px; }
.bar-1{background:#4B0082}
.bar-2{background:#4D0285}
.bar-3{background:#4E0587}
.bar-4{background:#50078A}
.bar-5{background:#51098C}
.bar-6{background:#530C8F}
.bar-7{background:#540E91}
.bar-8{background:#561194}
.bar-9{background:#571396}
.bar-10{background:#591599}
.bar-11{background:#5A189B}
.bar-12{background:#5C1A9E}

.pill-label { font-size:13px; font-weight:500;
  color:#64708b; letter-spacing:.01em; user-select:none;
  font-family:"DM Mono",monospace; display:none; }
"#;

// Mirrors indicator.js's animation (same smoothing/shimmer/bar math) so both
// waveforms move alike. Driven via `beamerSetState` + `beamerSetLevel` (~15Hz).
#[cfg(not(target_os = "linux"))]
pub(super) const PILL_JS: &str = r#"
(function() {
  var BAR_COUNT = 12, BAR_MIN_H = 4, BAR_MAX_H = 26, FRAME_MS = 33;
  window.__beamerLevel = 0;
  window.__beamerSmooth = 0;
  window.__beamerPhase = 0;
  window.__beamerState = null;

  window.beamerSetLevel = function(level) {
    window.__beamerLevel = Math.min(1, Math.max(0, level));
  };

  function animateBars() {
    var pill = document.querySelector('.pill');
    var bars = pill ? pill.querySelectorAll('.bar') : null;
    if (!pill || !bars || !bars.length) return;
    window.__beamerPhase += 0.35;
    var state = window.__beamerState;
    // Recording/note follow mic level; processing idles at a calm sweep.
    var target = (state === 'recording' || state === 'note')
      ? Math.max(0.12, window.__beamerLevel)
      : 0.15;
    var rate = target > window.__beamerSmooth ? 0.45 : 0.15;
    window.__beamerSmooth += (target - window.__beamerSmooth) * rate;
    for (var i = 0; i < BAR_COUNT; i++) {
      var shimmer = 0.35 + 0.65 * Math.abs(Math.sin(window.__beamerPhase + i * 0.55));
      var wave = 1 - window.__beamerSmooth * (1 - shimmer);
      var h = BAR_MIN_H + (BAR_MAX_H - BAR_MIN_H) * window.__beamerSmooth * wave;
      bars[i].style.height = Math.round(Math.min(BAR_MAX_H, h)) + 'px';
    }
  }

  function stopAnim() {
    if (window.__beamerAnim) { clearInterval(window.__beamerAnim); window.__beamerAnim = null; }
  }

  window.beamerSetState = function(state) {
    var pill = document.querySelector('.pill');
    var label = document.querySelector('.pill-label');
    if (!pill || !label) return;
    var wasVisible = window.__beamerState !== null;
    window.__beamerState = state === 'idle' ? null : state;

    pill.classList.toggle('pill-note', state === 'note');
    label.style.display = state === 'processing' ? '' : 'none';

    if (state === 'idle') {
      // 200ms ease-in, matching indicator.js hide().
      pill.style.transition = 'opacity 200ms cubic-bezier(0.55,0.085,0.68,0.53), '
        + 'transform 200ms cubic-bezier(0.55,0.085,0.68,0.53)';
      pill.style.opacity = '0';
      pill.style.transform = 'translateY(20px) scale(0.92)';
      stopAnim();
      window.__beamerSmooth = 0;
      return;
    }

    if (!window.__beamerAnim) {
      window.__beamerAnim = setInterval(animateBars, FRAME_MS);
    }
    if (!wasVisible) {
      // 250ms ease-out, matching indicator.js show().
      pill.style.transition = 'opacity 250ms cubic-bezier(0.25,0.46,0.45,0.94), '
        + 'transform 250ms cubic-bezier(0.25,0.46,0.45,0.94)';
      pill.style.opacity = '1';
      pill.style.transform = 'translateY(0) scale(1)';
    }
  };
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_capture_gets_its_own_pill_style() {
        assert_eq!(
            pill_state(RecordingState::Recording, CaptureMode::Note),
            Some("note"),
            "dictating into a note instead of the focused field must never be a surprise"
        );
        assert_eq!(
            pill_state(RecordingState::Recording, CaptureMode::Inject),
            Some("recording")
        );
    }

    #[test]
    fn transcribing_looks_the_same_whatever_the_destination() {
        assert_eq!(
            pill_state(RecordingState::Processing, CaptureMode::Note),
            Some("processing")
        );
        assert_eq!(
            pill_state(RecordingState::Processing, CaptureMode::Inject),
            Some("processing")
        );
    }

    #[test]
    fn idle_hides_the_pill_in_either_mode() {
        assert_eq!(pill_state(RecordingState::Idle, CaptureMode::Note), None);
        assert_eq!(pill_state(RecordingState::Idle, CaptureMode::Inject), None);
    }

    #[test]
    fn every_style_is_one_both_pills_handle() {
        // Unknown strings read as idle on both pills, silently. No error, no log.
        for state in [RecordingState::Idle, RecordingState::Recording, RecordingState::Processing] {
            for mode in [CaptureMode::Inject, CaptureMode::Note] {
                if let Some(style) = pill_state(state, mode) {
                    assert!(
                        matches!(style, "recording" | "processing" | "note"),
                        "{style:?} is not a state either pill handles"
                    );
                }
            }
        }
    }
}
