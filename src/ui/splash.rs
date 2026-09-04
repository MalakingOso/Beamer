use dioxus::prelude::*;

/// Loading splash while `warmup::warm_all` runs. Own transparent always-on-top
/// window; progress pushed in via `webview.evaluate_script` (pill pattern).
#[component]
pub fn SplashWindow() -> Element {
    let icon_data_url = crate::assets::icon_png_data_url();

    // Windows/macOS: dioxus hides non-first windows on load (see `ui::sticky`);
    // without this the splash never appears there.
    #[cfg(not(target_os = "linux"))]
    {
        let window = dioxus::desktop::use_window();
        use_effect(move || window.set_visible(true));
    }

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

/* 75% of card width, capsule (radius = half height). */
.splash-bar-track { width:180px; height:8px;
  background:#f1f3f9; border-radius:4px; overflow:hidden; }

/* Time-driven 0→100% over 1500ms (minimum-visible floor), not real progress. */
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
