#![cfg(not(target_os = "linux"))]
//! The day-end card: a gold-framed list of what you finished, in its own small
//! window. Opened by `app_day_end` at the set time (or by "Show it now"); what
//! it says is decided in `notes::day_end`.
//!
//! The card is the Done pill's gold ring grown into a list: Beamer Purple
//! chrome (2px border, hard purple shadow, no blur), gold only for the frame and
//! the line stripes. It never takes focus, does not sort your day into kinds,
//! and goes away with one click. The reveal is pure CSS: the frame slides in,
//! then the lines land 150 ms apart.

use std::time::Duration;

use dioxus::desktop::tao::dpi::{LogicalPosition, LogicalSize};
use dioxus::desktop::use_window;
use dioxus::prelude::*;

use crate::notes::day_end::CardView;
use crate::ui::icons::{IconCheck, IconX};

/// Card width, logical px.
pub const CARD_W: f64 = 380.0;
/// Room beside and below the card for its 4px/8px hard shadow.
pub const SHADOW: f64 = 12.0;
/// Gap between the window and the work-area corner.
pub const MARGIN: f64 = 20.0;
/// Tall enough for seven two-line rows; the card sizes the window to itself once mounted.
pub const INITIAL_H: f64 = 560.0;

#[derive(Props, Clone, PartialEq)]
pub struct DayEndCardProps {
    pub view: CardView,
    /// Bottom-right corner of the spot the card takes, logical px.
    pub anchor: (f64, f64),
    /// Seconds before it leaves by itself; 0 = stays until clicked.
    pub fade_secs: u32,
    /// Bumped to ask the main window to open the Done page.
    pub open_done: Signal<u32>,
}

#[component]
pub fn DayEndCard(props: DayEndCardProps) -> Element {
    let window = use_window();
    let mut shown = use_signal(|| false);
    let mut hovered = use_signal(|| false);
    let mut copied = use_signal(|| false);

    // Size the window to the card, tuck it into the corner, then show it: the
    // card's height depends on how its lines wrap, which only the webview knows.
    let on_mounted = {
        let window = window.clone();
        let anchor = props.anchor;
        move |e: MountedEvent| {
            let window = window.clone();
            async move {
                // If the measurement fails, keep the generous initial height: a
                // card with an empty strip above it beats one in the wrong corner.
                let h = match e.data().get_client_rect().await {
                    // A zero height means it was measured before layout.
                    Ok(rect) if rect.size.height > 0.0 => rect.size.height + SHADOW,
                    _ => INITIAL_H,
                };
                let w = CARD_W + SHADOW;
                window.set_inner_size(LogicalSize::new(w, h));
                window.set_outer_position(LogicalPosition::new(anchor.0 - w, anchor.1 - h));
                // Non-first windows start hidden on Windows/macOS (see `ui::splash`).
                window.set_visible(true);
                shown.set(true);
            }
        }
    };

    // Optional self-dismiss. Counts only while the card is up and not hovered,
    // matching the CSS edge, which pauses on hover.
    use_future({
        let window = window.clone();
        let fade = props.fade_secs;
        move || {
            let window = window.clone();
            async move {
                if fade == 0 {
                    return;
                }
                let mut left_ms = u64::from(fade) * 1000;
                while left_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    if *shown.peek() && !*hovered.peek() {
                        left_ms = left_ms.saturating_sub(250);
                    }
                }
                window.close();
            }
        }
    });

    let view = &props.view;
    let n = view.lines.len();
    let card_class = if *shown.read() { "de-card go" } else { "de-card" };
    let copy_text = view.copy_text.clone();

    rsx! {
        div {
            id: "de-card",
            class: "{card_class}",
            style: "--n:{n}",
            onmounted: on_mounted,
            onmouseenter: move |_| hovered.set(true),
            onmouseleave: move |_| hovered.set(false),
            // Anywhere that is not a footer action dismisses the card.
            onclick: {
                let window = window.clone();
                move |_| window.close()
            },
            div { class: "de-head",
                h1 { class: "de-title", "{view.heading}" }
                div { class: "de-cap",
                    span { "{view.caption}" }
                    span { "{view.count}" }
                }
                button {
                    class: "de-x",
                    aria_label: "Dismiss",
                    onclick: {
                        let window = window.clone();
                        move |e: MouseEvent| {
                            e.stop_propagation();
                            window.close();
                        }
                    },
                    IconX { size: 14 }
                }
            }
            div { class: "de-body",
                if view.earlier > 0 {
                    button {
                        class: "de-earlier",
                        onclick: {
                            let window = window.clone();
                            let mut open_done = props.open_done;
                            move |e: MouseEvent| {
                                e.stop_propagation();
                                open_done += 1;
                                window.close();
                            }
                        },
                        "{view.earlier} earlier"
                    }
                }
                for (i, line) in view.lines.iter().enumerate() {
                    div { key: "{i}", class: "de-ln", style: "--i:{i}",
                        b {}
                        i { "{line.time}" }
                        span { "{line.text}" }
                    }
                }
            }
            div { class: "de-ft",
                button {
                    onclick: move |e: MouseEvent| {
                        e.stop_propagation();
                        let text = copy_text.clone();
                        spawn(async move {
                            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                let _ = clipboard.set_text(&text);
                            }
                            copied.set(true);
                            tokio::time::sleep(Duration::from_millis(1500)).await;
                            copied.set(false);
                        });
                    },
                    if *copied.read() {
                        IconCheck { size: 13 }
                        "Copied"
                    } else {
                        "Copy list"
                    }
                }
                button {
                    onclick: {
                        let window = window.clone();
                        let mut open_done = props.open_done;
                        move |e: MouseEvent| {
                            e.stop_propagation();
                            open_done += 1;
                            window.close();
                        }
                    },
                    "Open Done"
                }
            }
            if props.fade_secs > 0 {
                div { class: "de-edge",
                    div { class: "de-fill", style: "animation-duration:{props.fade_secs}s" }
                }
            }
        }
    }
}

/// The card's CSS, mirroring the design boards: gold #E4A421 frame (the Done
/// pill's ring), pale gold #FFF6DC heading band, deep gold #8A5A00 for times
/// (gold itself fails contrast as text on white), Beamer Purple everywhere else.
pub const DAY_END_CSS: &str = r#"
html, body { margin:0; background:transparent; overflow:hidden;
  font-family:"Recursive","Segoe UI Variable","Segoe UI",system-ui,sans-serif; }
button { font:inherit; color:inherit; background:none; border:0; padding:0; cursor:pointer; }
.de-card { position:relative; width:380px; box-sizing:border-box; background:#fff;
  border:2px solid #E4A421; border-radius:6px; box-shadow:4px 8px 0 0 rgba(75,0,130,.15);
  overflow:hidden; color:#0f152a; cursor:default; user-select:none; opacity:0; }
.de-card.go { animation:de-in 200ms cubic-bezier(.25,1,.5,1) both; }
.de-head { position:relative; background:#FFF6DC; border-bottom:2px solid #E4A421; padding:12px 16px 10px; }
.de-title { margin:0; font-family:"DM Mono",monospace; font-size:17px; font-weight:500; line-height:1.25; }
.de-cap { display:flex; justify-content:space-between; margin-top:2px;
  font-family:"DM Mono",monospace; font-size:11px; color:#8A5A00; }
.de-x { position:absolute; right:10px; top:9px; color:#4B0082; opacity:0; display:flex;
  transition:opacity 150ms cubic-bezier(.25,1,.5,1); }
.de-card:hover .de-x, .de-x:focus-visible { opacity:1; }
.de-body { padding:4px 0; }
.de-earlier { display:block; padding:3px 16px 3px 60px; font-family:"DM Mono",monospace;
  font-size:11px; font-weight:500; color:#4B0082; }
.de-ln { display:flex; gap:10px; align-items:stretch; padding:5px 16px; font-size:13px; line-height:1.35; }
.de-ln b { width:4px; flex:none; border-radius:2px; background:#E4A421;
  transform-origin:top; transform:scaleY(0); }
.de-ln i { flex:none; width:34px; padding-top:2px; font-style:normal;
  font-family:"DM Mono",monospace; font-size:11px; color:#8A5A00; opacity:0; }
.de-ln span { min-width:0; opacity:0; display:-webkit-box; -webkit-line-clamp:2;
  -webkit-box-orient:vertical; overflow:hidden; }
.go .de-ln b { animation:de-stripe 150ms cubic-bezier(.25,1,.5,1) forwards;
  animation-delay:calc(250ms + var(--i) * 150ms); }
.go .de-ln i, .go .de-ln span { animation:de-fade 150ms cubic-bezier(.25,1,.5,1) forwards;
  animation-delay:calc(325ms + var(--i) * 150ms); }
.de-ft { display:flex; gap:16px; border-top:2px solid rgba(75,0,130,.12); padding:8px 16px;
  font-size:12px; font-weight:500; color:#4B0082; opacity:0; }
.de-ft button { display:flex; align-items:center; gap:5px; }
.de-ft button:hover { text-decoration:underline; }
.go .de-ft { animation:de-fade 200ms cubic-bezier(.25,1,.5,1) forwards;
  animation-delay:calc(325ms + var(--n) * 150ms); }
.de-edge { height:4px; background:rgba(228,164,33,.25); }
.de-fill { height:4px; background:#E4A421; animation:de-drain linear forwards; }
.de-card:hover .de-fill { animation-play-state:paused; }
@keyframes de-in { from { opacity:0; transform:translateX(24px); } to { opacity:1; transform:none; } }
@keyframes de-stripe { to { transform:scaleY(1); } }
@keyframes de-fade { to { opacity:1; } }
@keyframes de-drain { from { width:100%; } to { width:0; } }
@media (prefers-reduced-motion: reduce) {
  .de-card, .de-card * { animation:none !important; transition:none !important; }
  .de-card, .de-ln i, .de-ln span, .de-ft { opacity:1; }
  .de-ln b { transform:none; }
  .de-edge { display:none; }
}
"#;
