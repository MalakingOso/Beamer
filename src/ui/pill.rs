use dioxus::prelude::*;

use crate::hotkey::CaptureMode;
use crate::orchestrator::RecordingState;

/// Which pill style a recording state and capture mode select, or `None` to
/// hide the pill. Shared by both platforms' pills: `linux_integration.rs`
/// passes the result to the GNOME extension's `indicator.js` (which branches
/// on these exact strings, so a typo here would look like a working pill
/// that simply ignores your microphone), and `app_setup.rs` passes it to
/// this file's `beamerSetState`. One function means the two pills can never
/// drift on which states exist or what they're called.
///
/// Factored out so it can be tested without a live Dioxus runtime or a
/// window/D-Bus connection.
pub(super) fn pill_state(state: RecordingState, mode: CaptureMode) -> Option<&'static str> {
    match (state, mode) {
        (RecordingState::Idle, _) => None,
        (RecordingState::Recording, CaptureMode::Note) => Some("note"),
        (RecordingState::Recording, CaptureMode::Inject) => Some("recording"),
        // Transcribing looks the same either way. The destination is already
        // decided by this point and the pill's only job is to say "working".
        (RecordingState::Processing, _) => Some("processing"),
    }
}

/// A small Deploy Purple pill shown at bottom-center of the screen while
/// recording: a 12-bar purple-gradient waveform that follows the live mic
/// level, plus "Transcribing…" while processing. Deliberately has no
/// elapsed-time readout — the GNOME-Shell-drawn pill this mirrors
/// (`extension/beamer-focus@beamer.app/indicator.js`) never had one, and
/// matching it means matching what it leaves out too. Rendered in its own
/// transparent, click-through, always-on-top window; state and level updates
/// are driven from app_setup.rs via the `beamerSetState`/`beamerSetLevel`
/// head-script.
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

/// Logical-pixel size of the pill's own window, and where the pill's ink
/// stops inside it.
///
/// The window is a viewport, not a canvas: `html, body` are `overflow:hidden`,
/// so every pixel `PILL_CSS` draws past the window's edge is silently cut.
/// That is all a flat, borderless bottom edge on the pill ever was — the
/// window used to be 52px tall, four short of the pill's own border box:
///
/// ```text
///    4  .pill margin-top
///   48  .pill height (content-box: the reset zeroes margin/padding, not box-sizing)
///    4  two 2px borders
///  ----
///   56  resting border box, bottom border edge
///    4  the box-shadow's 4px y-offset
///  ----
///   60  = PILL_INK_BOTTOM, every pixel the pill paints at rest
///   20  beamerSetState's entrance translateY(20px), painted before it settles
///  ----
///   80  = PILL_WINDOW_H
/// ```
///
/// Width is sized for the widest state rather than the idle one: `processing`
/// un-hides the `Transcribing…` label, which puts the row at roughly 224px
/// including the shadow, over the 220px the window used to be.
///
/// Change either of these and the two numbers below have to move with them,
/// or the pill loses an edge again with no error and no log line.
#[cfg(not(target_os = "linux"))]
pub(super) const PILL_WINDOW_W: f64 = 264.0;
#[cfg(not(target_os = "linux"))]
pub(super) const PILL_WINDOW_H: f64 = 80.0;
#[cfg(not(target_os = "linux"))]
pub(super) const PILL_INK_BOTTOM: f64 = 60.0;

// Deploy Purple light card: solid light surface, structural 2px border,
// sharp 8px radius, hard-offset shadow — the same tokens, the same shape,
// and (bar-for-bar) the same waveform gradient as the GNOME-Shell-drawn pill
// in extension/beamer-focus@beamer.app/{stylesheet.css,indicator.js}, so the
// two platforms' pills read as one design instead of two. The 12 `.bar-N`
// colors below are `indicator.js`'s COLOR_FROM (#4B0082, --accent) to
// COLOR_TO (#5C1A9E, --accent-hover) lerp, computed the same way: `c_from +
// (c_to - c_from) * i/11`, rounded.
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
  /* Bottom-anchored, matching indicator.js's `set_pivot_point(0.5, 1.0)`:
     the entrance/exit scale grows from the bottom edge, not the center. */
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

// Mirrors indicator.js's `show()`/`hide()`/`setLevel()`/`_animateBars()`
// exactly (same smoothing constants, same shimmer formula, same bar-height
// math) so the two waveforms move the same way, not just wear the same
// colors. app_setup.rs calls `beamerSetState('recording'|'note'|'processing'|
// 'idle')` on every state change and `beamerSetLevel(level)` at ~15Hz while
// recording, same cadence `linux_integration.rs` pumps into the shell pill.
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
    // Recording/note follow the live mic level; processing idles at a calm
    // constant sweep, same as indicator.js's _animateBars.
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
      // 200ms ease-in, matching indicator.js's hide() (Clutter EASE_IN_QUAD).
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
      // 250ms ease-out, matching indicator.js's show() (Clutter EASE_OUT_QUAD).
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
        // indicator.js (Linux) and beamerSetState (Windows/macOS) both branch
        // on these exact strings and silently treat an unknown one as "not
        // recording" — a labelled idle sweep that ignores the microphone.
        // That failure has no error and no log line on either platform.
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
