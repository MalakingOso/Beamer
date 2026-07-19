use dioxus::prelude::*;

/// Loading splash shown while `warmup::warm_all` runs on app startup.
/// Lives in its own transparent, always-on-top window. Progress updates are
/// pushed in by the parent vdom via `webview.evaluate_script` (same pattern
/// as the recording pill — see `src/ui/app.rs`).
#[component]
pub fn SplashWindow() -> Element {
    let icon_data_url = crate::assets::icon_png_data_url();

    rsx! {
        div { class: "splash-root",
            div { class: "splash-card",
                img { class: "splash-icon", src: "{icon_data_url}" }
                div { class: "splash-bar-track",
                    div { class: "splash-bar-fill" }
                }
                div { class: "splash-label", "Starting…" }
            }
        }
    }
}

pub const SPLASH_CSS: &str = r#"
*, *::before, *::after { margin:0; padding:0; box-sizing:border-box; }
html, body, #main { background:transparent!important; overflow:hidden;
  font-family:"Recursive","Segoe UI Variable","Segoe UI",system-ui,sans-serif;
  color:#0f152a; }

.splash-root { width:100vw; height:100vh; display:flex;
  align-items:center; justify-content:center; }

.splash-card { width:240px; height:240px; padding:24px;
  background:#ffffff;
  border:2px solid rgba(75,0,130,0.25);
  border-radius:12px;
  box-shadow:4px 8px 0 0 rgba(75,0,130,0.18);
  display:flex; flex-direction:column;
  align-items:center; justify-content:center;
  gap:14px; }

.splash-icon { width:128px; height:128px; border-radius:12px;
  user-select:none; -webkit-user-drag:none; }

/* 75% of the 240px card width, capsule shape (radius = half the height). */
.splash-bar-track { width:180px; height:8px;
  background:#f1f3f9; border-radius:4px; overflow:hidden; }

/* Bar fill is purely time-driven: 0 → 100% over 1500ms, matching the
   splash's minimum-visible floor in app.rs. Decoupled from real warmup
   progress so the user always sees a smooth fill regardless of cold/warm. */
.splash-bar-fill { height:100%; width:0%; background:#4B0082;
  border-radius:4px;
  animation:splash-fill 1500ms cubic-bezier(0.4, 0, 0.2, 1) forwards; }

@keyframes splash-fill {
  from { width:0%; }
  to   { width:100%; }
}

.splash-label { font-size:12px; color:#94a0b8; letter-spacing:0.01em;
  user-select:none; }
"#;
