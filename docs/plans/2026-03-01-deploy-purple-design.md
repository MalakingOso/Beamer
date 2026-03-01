# Deploy Purple Design Language Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Migrate Beamer's UI from "Developer Precision" to "Deploy Purple" — the Deploy Blue design system adapted with acrylic blur backgrounds and #4B0082 purple accent.

**Architecture:** CSS-only redesign (new tokens, typography, borders, shadows) plus two small Rust changes: enabling window transparency for acrylic and calling DwmSetWindowAttribute for the system backdrop. Font files bundled in assets/fonts/.

**Tech Stack:** CSS custom properties, DM Mono + Recursive fonts (woff2), Windows DWM API via `windows` crate, Dioxus desktop `with_transparent` + `with_background_color`.

---

### Task 1: Download and bundle font files

**Files:**
- Create: `assets/fonts/DMMono-Regular.woff2`
- Create: `assets/fonts/DMMono-Medium.woff2`
- Create: `assets/fonts/Recursive-Variable.woff2`

**Step 1: Download DM Mono from Google Fonts**

Download DM Mono Regular (400) and Medium (500) as woff2 files. These are available from Google Fonts CDN. Use the google-webfonts-helper or download directly:

```bash
# DM Mono Regular
curl -o assets/fonts/DMMono-Regular.woff2 "https://fonts.gstatic.com/s/dmmono/v14/aFTU7PB1QTsUX8KYhh2aBYyMcKw.woff2"

# DM Mono Medium
curl -o assets/fonts/DMMono-Medium.woff2 "https://fonts.gstatic.com/s/dmmono/v14/aFTR7PB1QTsUX8KYvrGyIYSnbKX9Aw.woff2"

# Recursive Variable (full variable font — covers Regular through Bold, plus MONO axis)
curl -o assets/fonts/Recursive-Variable.woff2 "https://fonts.gstatic.com/s/recursive/v44/8vJN7wMr0mhh-RQChyHEH06TlXhq_gukbYrFMk1QuAIcyEwG_X-dpEfaE5YaERmK-CImKsvxvR-CIg.woff2"
```

**Step 2: Verify files exist**

```bash
ls -la assets/fonts/
```

Expected: Three .woff2 files, each 10-80KB.

**Step 3: Commit**

```bash
git add assets/fonts/
git commit -m "chore: add DM Mono and Recursive font files for Deploy Purple"
```

---

### Task 2: Rewrite CSS — tokens, fonts, and base styles

**Files:**
- Modify: `assets/styles.css` (full rewrite)

**Step 1: Replace the entire CSS file**

Replace `assets/styles.css` with the complete Deploy Purple stylesheet. The full CSS follows. Key changes from the old file:

- `:root` tokens: new 4-level text hierarchy, purple-tinted borders, hard-offset shadow tokens, radius tokens, motion tokens
- `@font-face` declarations for DM Mono and Recursive
- `body`: Recursive font, antialiased rendering, transparent bg
- All borders changed from `0.5px` to `2px solid var(--border)`
- Card titles: DM Mono 17px bold (was Geist 12px uppercase)
- Buttons: 2px borders, 8px 16px padding, 4px radius, primary gets hard shadow
- Inputs/selects: 2px borders, 8px 12px padding
- Toggles: 2px borders
- Sidebar: 2px border-right, accent-subtle active background
- Hard offset shadows on card hover and primary buttons
- All scrollbar, status-log, history, overlay, glow styles updated

```css
/* Beamer — Deploy Purple Design System */

/* ===== Font Loading ===== */
@font-face {
  font-family: "DM Mono";
  src: url("fonts/DMMono-Regular.woff2") format("woff2");
  font-weight: 400;
  font-style: normal;
  font-display: swap;
}

@font-face {
  font-family: "DM Mono";
  src: url("fonts/DMMono-Medium.woff2") format("woff2");
  font-weight: 500;
  font-style: normal;
  font-display: swap;
}

/* ===== Tokens ===== */
:root {
  /* Backgrounds — transparent for acrylic blur */
  --bg: transparent;
  --bg-surface: rgba(255, 255, 255, 0.72);
  --bg-hover: rgba(241, 243, 249, 0.80);
  --bg-active: rgba(232, 235, 244, 0.85);
  --bg-recessed: rgba(241, 243, 249, 0.50);

  /* Borders — purple-tinted, structural */
  --border: rgba(75, 0, 130, 0.12);
  --border-strong: rgba(75, 0, 130, 0.25);

  /* Text — 4-level ink hierarchy */
  --fg: #0f152a;
  --fg-secondary: #64708b;
  --fg-muted: #94a0b8;
  --fg-faint: #b8c0d4;

  /* Accent */
  --accent: #4B0082;
  --accent-hover: #5C1A9E;
  --accent-subtle: rgba(75, 0, 130, 0.08);
  --accent-wash: rgba(75, 0, 130, 0.04);

  /* Status */
  --danger: #DC2626;
  --danger-subtle: rgba(220, 38, 38, 0.06);
  --success: #16A34A;

  /* Radii */
  --radius: 4px;
  --radius-md: 6px;
  --radius-lg: 8px;

  /* Shadows — hard offset, zero blur */
  --shadow-card: 2px 4px 0 0 rgba(75, 0, 130, 0.12);
  --shadow-cta: 2px 4px 0 0 #4a4a4a, 0 0 0 1px #4B0082;
  --shadow-modal: 4px 8px 0 0 rgba(75, 0, 130, 0.15), 0 0 0 2px var(--border);

  /* Motion */
  --duration-fast: 150ms;
  --duration: 200ms;
  --ease: cubic-bezier(0.25, 1, 0.5, 1);
}

/* ===== Reset ===== */
* {
  margin: 0;
  padding: 0;
  box-sizing: border-box;
}

::selection {
  background: rgba(75, 0, 130, 0.16);
}

/* ===== Body ===== */
body {
  font-family: "Recursive", "Segoe UI Variable", "Segoe UI", system-ui, sans-serif;
  color: var(--fg);
  background: var(--bg);
  font-size: 14px;
  line-height: 1.55;
  overflow: hidden;
  -webkit-font-smoothing: antialiased;
  text-rendering: optimizeLegibility;
}

/* ===== Title Bar ===== */
.titlebar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  height: 32px;
  padding: 0 8px 0 16px;
  border-bottom: 2px solid var(--border);
  background: var(--bg-surface);
  -webkit-app-region: drag;
  user-select: none;
}

.titlebar-title {
  font-family: "DM Mono", "Cascadia Code", monospace;
  font-size: 13px;
  font-weight: 500;
  color: var(--fg);
  letter-spacing: -0.01em;
}

.titlebar-controls {
  display: flex;
  gap: 0;
  -webkit-app-region: no-drag;
}

.titlebar-btn {
  width: 32px;
  height: 32px;
  border: none;
  background: transparent;
  color: var(--fg-muted);
  font-size: 12px;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  border-radius: 0;
  transition: background var(--duration-fast) var(--ease), color var(--duration-fast) var(--ease);
}

.titlebar-btn:hover {
  background: var(--bg-hover);
  color: var(--fg);
}

.titlebar-btn.close:hover {
  background: var(--danger);
  color: white;
}

/* ===== Layout Shell ===== */
.app-container {
  display: flex;
  flex-direction: column;
  height: 100vh;
  overflow: hidden;
}

.app-body {
  display: flex;
  flex-direction: row;
  flex: 1;
  overflow: hidden;
}

/* ===== Sidebar ===== */
.sidebar {
  width: 44px;
  min-width: 44px;
  display: flex;
  flex-direction: column;
  background: var(--bg-surface);
  border-right: 2px solid var(--border);
}

.sidebar-top {
  display: flex;
  flex-direction: column;
  flex: 1;
  padding-top: 4px;
}

.sidebar-bottom {
  display: flex;
  flex-direction: column;
  padding-bottom: 4px;
}

.sidebar-icon {
  width: 44px;
  height: 44px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--fg-muted);
  cursor: pointer;
  border: none;
  background: transparent;
  position: relative;
  transition: color var(--duration-fast) var(--ease), background var(--duration-fast) var(--ease);
}

.sidebar-icon:hover {
  background: var(--bg-hover);
  color: var(--fg-secondary);
}

.sidebar-icon.active {
  color: var(--accent);
  background: var(--accent-subtle);
}

.sidebar-icon.active::before {
  content: "";
  position: absolute;
  left: 0;
  top: 8px;
  bottom: 8px;
  width: 2px;
  background: var(--accent);
  border-radius: 0 1px 1px 0;
}

/* ===== Content Area ===== */
.content {
  flex: 1;
  overflow-y: auto;
  padding: 24px 28px;
  display: flex;
  flex-direction: column;
  gap: 20px;
}

/* ===== Scrollbar ===== */
.content::-webkit-scrollbar {
  width: 6px;
}

.content::-webkit-scrollbar-track {
  background: transparent;
}

.content::-webkit-scrollbar-thumb {
  background: rgba(0, 0, 0, 0.10);
  border-radius: 3px;
}

.content::-webkit-scrollbar-thumb:hover {
  background: rgba(0, 0, 0, 0.18);
}

/* ===== Card ===== */
.card {
  background: var(--bg-surface);
  border: 2px solid var(--border);
  border-radius: var(--radius-md);
  padding: 16px;
  transition: box-shadow var(--duration-fast) var(--ease);
}

.card:hover {
  box-shadow: var(--shadow-card);
}

.card-title {
  font-family: "DM Mono", "Cascadia Code", monospace;
  font-size: 17px;
  font-weight: 500;
  color: var(--fg);
  letter-spacing: -0.025em;
  margin-bottom: 12px;
}

.card-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  min-height: 32px;
  gap: 12px;
}

.card-row + .card-row {
  margin-top: 8px;
}

.card-label {
  font-size: 13px;
  color: var(--fg-secondary);
  min-width: 80px;
}

.card-label-hint {
  font-size: 12px;
  color: var(--fg-muted);
  font-style: italic;
}

/* ===== Select ===== */
.select {
  appearance: none;
  background: var(--bg-surface);
  border: 2px solid var(--border);
  border-radius: var(--radius);
  padding: 6px 28px 6px 12px;
  font-size: 13px;
  font-weight: 500;
  color: var(--fg);
  cursor: pointer;
  min-width: 180px;
  font-family: inherit;
  background-image: url("data:image/svg+xml,%3Csvg width='10' height='6' viewBox='0 0 10 6' fill='none' xmlns='http://www.w3.org/2000/svg'%3E%3Cpath d='M1 1L5 5L9 1' stroke='%2394a0b8' stroke-width='1.5' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E");
  background-repeat: no-repeat;
  background-position: right 10px center;
  transition: border-color var(--duration-fast) var(--ease);
}

.select:hover {
  border-color: var(--border-strong);
}

.select:focus {
  outline: none;
  border-color: var(--accent);
}

/* ===== Input ===== */
.input {
  background: var(--bg-surface);
  border: 2px solid var(--border);
  border-radius: var(--radius);
  padding: 6px 12px;
  font-size: 13px;
  color: var(--fg);
  font-family: inherit;
  min-width: 120px;
  transition: border-color var(--duration-fast) var(--ease);
}

.input:hover {
  border-color: var(--border-strong);
}

.input:focus {
  outline: none;
  border-color: var(--accent);
}

.input::placeholder {
  color: var(--fg-faint);
}

.input-mono {
  font-family: "Cascadia Code", "JetBrains Mono", "Consolas", monospace;
  font-variant-numeric: tabular-nums;
}

/* ===== Masked Input ===== */
.masked-container {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: 1;
}

.masked-container .input {
  flex: 1;
}

/* ===== Toggle ===== */
.toggle {
  position: relative;
  width: 36px;
  height: 20px;
  border-radius: 10px;
  border: 2px solid var(--border);
  background: transparent;
  cursor: pointer;
  transition: background-color var(--duration-fast) var(--ease), border-color var(--duration-fast) var(--ease);
}

.toggle.active {
  background: var(--accent);
  border-color: var(--accent);
}

.toggle-knob {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 12px;
  height: 12px;
  border-radius: 6px;
  background: var(--fg-muted);
  transition: transform var(--duration-fast) var(--ease), background-color var(--duration-fast) var(--ease);
}

.toggle.active .toggle-knob {
  transform: translateX(16px);
  background: white;
}

/* ===== Radio ===== */
.radio-group {
  display: flex;
  gap: 16px;
}

.radio-option {
  display: flex;
  align-items: center;
  gap: 6px;
  cursor: pointer;
  font-size: 13px;
  color: var(--fg-secondary);
}

.radio-dot {
  width: 14px;
  height: 14px;
  border-radius: 7px;
  border: 2px solid var(--border);
  display: flex;
  align-items: center;
  justify-content: center;
  transition: border-color var(--duration-fast) var(--ease);
}

.radio-dot.selected {
  border-color: var(--accent);
}

.radio-dot.selected::after {
  content: "";
  width: 6px;
  height: 6px;
  border-radius: 3px;
  background: var(--accent);
}

/* ===== Tag Chips ===== */
.tag-list {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-bottom: 8px;
}

.tag-chip {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  border: 2px solid var(--border);
  border-radius: var(--radius);
  font-family: "Cascadia Code", "JetBrains Mono", "Consolas", monospace;
  font-size: 12px;
  color: var(--fg-secondary);
  font-variant-numeric: tabular-nums;
}

.tag-remove {
  cursor: pointer;
  color: var(--fg-faint);
  font-size: 10px;
  border: none;
  background: none;
  padding: 0 2px;
  transition: color var(--duration-fast) var(--ease);
}

.tag-remove:hover {
  color: var(--danger);
}

/* ===== Buttons ===== */
.btn {
  padding: 8px 16px;
  border-radius: var(--radius);
  border: 2px solid var(--border);
  background: var(--bg-surface);
  color: var(--fg-secondary);
  font-size: 13px;
  font-weight: 500;
  cursor: pointer;
  font-family: inherit;
  transition: background var(--duration-fast) var(--ease), border-color var(--duration-fast) var(--ease), color var(--duration-fast) var(--ease);
}

.btn:hover {
  background: var(--bg-hover);
  border-color: var(--border-strong);
}

.btn:active {
  background: var(--bg-active);
}

.btn:disabled {
  opacity: 0.35;
  cursor: not-allowed;
}

.btn-primary {
  background: var(--accent);
  color: white;
  border-color: var(--accent);
  box-shadow: var(--shadow-cta);
}

.btn-primary:hover {
  background: var(--accent-hover);
  border-color: var(--accent-hover);
}

.btn-primary:active {
  box-shadow: none;
  transform: translate(1px, 2px);
}

.btn-small {
  padding: 4px 12px;
  font-size: 12px;
}

.btn-text {
  border: none;
  padding: 2px 6px;
  color: var(--accent);
  background: none;
  box-shadow: none;
}

.btn-text:hover {
  color: var(--accent-hover);
  background: none;
  border-color: transparent;
}

/* ===== Footer ===== */
.footer {
  padding: 12px 16px;
  border-top: 2px solid var(--border);
  display: flex;
  justify-content: center;
}

/* ===== Debug ===== */
.debug-text {
  font-family: "Cascadia Code", "JetBrains Mono", "Consolas", monospace;
  font-size: 12px;
  color: var(--fg-muted);
  padding: 4px 0;
  font-variant-numeric: tabular-nums;
}

/* ===== Color Preview ===== */
.color-preview {
  width: 20px;
  height: 20px;
  border-radius: var(--radius);
  border: 2px solid var(--border);
  display: inline-block;
  vertical-align: middle;
}

/* ===== Hotkey Display ===== */
.hotkey-display {
  font-family: "Cascadia Code", "JetBrains Mono", "Consolas", monospace;
  font-size: 12px;
  padding: 2px 8px;
  border: 2px solid var(--border);
  border-radius: var(--radius);
  color: var(--fg-secondary);
  background: var(--bg-recessed);
  font-variant-numeric: tabular-nums;
}

/* ===== Overlay Window ===== */
.overlay {
  position: fixed;
  bottom: 40px;
  left: 50%;
  transform: translateX(-50%);
  background: rgba(0, 0, 0, 0.75);
  color: white;
  font-family: "Cascadia Code", "JetBrains Mono", "Consolas", monospace;
  font-size: 14px;
  padding: 12px 20px;
  border-radius: var(--radius-lg);
  max-width: 500px;
  text-align: center;
}

/* ===== Glow Window ===== */
.glow {
  position: fixed;
  top: 0;
  left: 0;
  width: 100vw;
  height: 100vh;
  pointer-events: none;
  box-shadow: inset 0 0 8px 3px var(--glow-color, #4B0082);
}

/* ===== Status Dot ===== */
.status-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
}

.status-ready {
  background: var(--success);
}

.status-recording {
  background: var(--danger);
  animation: pulse 1.5s ease-in-out infinite;
}

.status-processing {
  background: #EAB308;
}

@keyframes pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.4; }
}

/* ===== Quick Settings ===== */
.quick-settings-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  min-height: 32px;
  gap: 12px;
}

.quick-settings-row + .quick-settings-row {
  margin-top: 8px;
}

/* ===== History ===== */
.history-group {
  margin-bottom: 8px;
}

.history-date-header {
  font-family: "DM Mono", "Cascadia Code", monospace;
  font-size: 11px;
  font-weight: 500;
  text-transform: uppercase;
  letter-spacing: 0.04em;
  color: var(--fg-muted);
  padding: 8px 0 4px;
}

.history-entry {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  padding: 6px 8px;
  border-radius: var(--radius);
  cursor: pointer;
  transition: background var(--duration-fast) var(--ease);
}

.history-entry:hover {
  background: var(--bg-hover);
}

.history-entry .copy-btn {
  opacity: 0;
  color: var(--fg-muted);
  font-size: 12px;
  flex-shrink: 0;
  margin-left: auto;
  transition: opacity var(--duration-fast) var(--ease);
}

.history-entry:hover .copy-btn {
  opacity: 1;
}

.entry-time {
  font-family: "Cascadia Code", "JetBrains Mono", "Consolas", monospace;
  font-size: 11px;
  color: var(--fg-muted);
  flex-shrink: 0;
  padding-top: 1px;
  font-variant-numeric: tabular-nums;
}

.entry-text {
  font-size: 13px;
  color: var(--fg-secondary);
  line-height: 1.4;
  word-break: break-word;
}

/* ===== Empty State ===== */
.empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  padding: 56px 32px;
  color: var(--fg-faint);
  text-align: center;
}

.empty-state-icon {
  font-size: 28px;
  margin-bottom: 8px;
  opacity: 0.4;
}

.empty-state-text {
  font-family: "DM Mono", "Cascadia Code", monospace;
  font-size: 17px;
  font-weight: 500;
  color: var(--fg-muted);
}

.empty-state-hint {
  font-size: 13px;
  color: var(--fg-muted);
  margin-top: 4px;
  line-height: 1.5;
}

/* ===== Status Log ===== */
.status-log {
  margin-top: 8px;
  max-height: 200px;
  overflow-y: auto;
  border: 2px solid var(--border);
  border-radius: var(--radius-md);
  padding: 4px;
  background: var(--bg-recessed);
}

.status-log::-webkit-scrollbar {
  width: 4px;
}

.status-log::-webkit-scrollbar-track {
  background: transparent;
}

.status-log::-webkit-scrollbar-thumb {
  background: rgba(0, 0, 0, 0.10);
  border-radius: 2px;
}

.log-entry {
  display: flex;
  align-items: baseline;
  gap: 6px;
  padding: 1px 4px;
  font-family: "Cascadia Code", "JetBrains Mono", "Consolas", monospace;
  font-size: 11px;
  line-height: 1.6;
  font-variant-numeric: tabular-nums;
}

.log-time {
  color: var(--fg-faint);
  flex-shrink: 0;
}

.log-level {
  flex-shrink: 0;
  font-weight: 600;
}

.log-level-info {
  color: var(--fg-muted);
}

.log-level-warn {
  color: #CA8A04;
}

.log-level-error {
  color: var(--danger);
}

.log-msg {
  color: var(--fg-secondary);
  word-break: break-word;
}

.log-empty {
  color: var(--fg-muted);
  font-size: 12px;
  padding: 8px;
  text-align: center;
}

/* ===== Focus & Accessibility ===== */
:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}

:focus:not(:focus-visible) {
  outline: none;
}
```

**Step 2: Verify the build compiles**

Run: `cargo build 2>&1 | tail -5`
Expected: Successful compilation (CSS changes don't affect Rust build, but verifies asset path still works).

**Step 3: Commit**

```bash
git add assets/styles.css
git commit -m "feat: rewrite CSS to Deploy Purple design language"
```

---

### Task 3: Enable window transparency and acrylic backdrop

**Files:**
- Modify: `src/ui/mod.rs:16-30` (add transparent window + background color)
- Modify: `src/ui/app.rs:1-6` (add tao platform import)
- Modify: `src/ui/app.rs:27-35` (add acrylic backdrop use_hook)

**Step 1: Update window config in `src/ui/mod.rs`**

Change the `launch_app()` function to enable window transparency and set webview background to transparent:

```rust
pub fn launch_app() {
    LaunchBuilder::new()
        .with_cfg(
            Config::new()
                .with_window(
                    WindowBuilder::new()
                        .with_title("Beamer")
                        .with_visible(false)
                        .with_decorations(false)
                        .with_transparent(true)
                        .with_inner_size(dioxus::desktop::LogicalSize::new(500.0_f64, 600.0_f64)),
                )
                .with_background_color((0, 0, 0, 0))
                .with_close_behaviour(WindowCloseBehaviour::WindowHides)
                .with_exits_when_last_window_closes(false),
        )
        .launch(app::App);
}
```

The two additions are:
- `.with_transparent(true)` on the WindowBuilder — tells tao to make the window transparent
- `.with_background_color((0, 0, 0, 0))` on the Dioxus Config — makes the webview2 background transparent

**Step 2: Add acrylic backdrop in `src/ui/app.rs`**

Add the import at the top of app.rs:

```rust
use dioxus::desktop::tao::platform::windows::WindowExtWindows;
```

Then inside the `App()` component, after `let window = use_window();`, add this hook to enable the acrylic system backdrop:

```rust
// Enable acrylic system backdrop via DWM
use_hook({
    let window = window.clone();
    move || {
        let hwnd = window.hwnd() as isize;
        unsafe {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_SYSTEMBACKDROP_TYPE};
            let backdrop_type: i32 = 3; // DWMSBT_TRANSIENTWINDOW = Acrylic
            let _ = DwmSetWindowAttribute(
                HWND(hwnd as *mut _),
                DWMWA_SYSTEMBACKDROP_TYPE,
                &backdrop_type as *const _ as *const c_void,
                std::mem::size_of::<i32>() as u32,
            );
        }
    }
});
```

Also add at the top of the file:

```rust
use std::ffi::c_void;
```

**Step 3: Verify it compiles**

Run: `cargo build 2>&1 | tail -10`
Expected: Successful compilation. The `windows` crate already has `Win32_Graphics_Dwm` in Cargo.toml features.

**Step 4: Test the app**

Run: `cargo run`
Expected: Window should show with acrylic blur effect behind semi-transparent surfaces. If Windows 11, full acrylic blur. If not supported, falls back to opaque (the `let _ =` ignores errors gracefully).

**Step 5: Commit**

```bash
git add src/ui/mod.rs src/ui/app.rs
git commit -m "feat: enable acrylic blur backdrop via DWM API"
```

---

### Task 4: Update design system documentation

**Files:**
- Modify: `agent_docs/design_system.md` (full rewrite)
- Modify: `CLAUDE.md:27` (update design language reference)

**Step 1: Rewrite `agent_docs/design_system.md`**

Replace the entire file to document the Deploy Purple system:

```markdown
# Design System — Deploy Purple

## Philosophy

Deploy Blue system adapted with acrylic blur and purple accent. Hard edges, structural borders, monospace display type, hard-offset shadows. Single accent (#4B0082). Micro-motion only.

## Color Tokens

```css
:root {
  --bg: transparent;                      /* Acrylic blur shows through */
  --bg-surface: rgba(255, 255, 255, 0.72); /* Cards, panels, inputs */
  --bg-hover: rgba(241, 243, 249, 0.80);   /* Interactive hover */
  --bg-active: rgba(232, 235, 244, 0.85);  /* Pressed/active */
  --bg-recessed: rgba(241, 243, 249, 0.50);/* Sunken areas */

  --border: rgba(75, 0, 130, 0.12);        /* 2px solid everywhere */
  --border-strong: rgba(75, 0, 130, 0.25); /* Focus rings, emphasis */

  --fg: #0f152a;          /* Primary text */
  --fg-secondary: #64708b; /* Body, descriptions */
  --fg-muted: #94a0b8;    /* Labels, metadata */
  --fg-faint: #b8c0d4;    /* Placeholders, disabled */

  --accent: #4B0082;
  --accent-hover: #5C1A9E;
  --accent-subtle: rgba(75, 0, 130, 0.08);

  --danger: #DC2626;
  --success: #16A34A;
}
```

## Typography

Three layers:
1. **Display/Headers:** `DM Mono` — monospace character, 17px weight 500 for section headings
2. **UI Chrome:** `Recursive` — variable sans, 13-14px for body/buttons/labels
3. **Data/Debug:** `Cascadia Code` / `JetBrains Mono` — monospace, tabular-nums

## Spacing

4px base grid:
- 8px — within components
- 12px — standard gap
- 16px — card padding
- 20px — between cards in content
- 24-28px — content area padding

## Borders

All borders: `2px solid var(--border)`. No thin borders. Visible and structural.

## Shadows

Hard offset, zero blur:
- Cards on hover: `2px 4px 0 0 rgba(75,0,130,0.12)`
- Primary buttons: `2px 4px 0 0 #4a4a4a, 0 0 0 1px #4B0082`
- Modals: `4px 8px 0 0 rgba(75,0,130,0.15), 0 0 0 2px var(--border)`

## Border Radius

Sharp system: 4px base (--radius), 6px cards (--radius-md), 8px max (--radius-lg). Never rounder.

## Motion

- 150ms for hover states (--duration-fast)
- 200ms for layout transitions (--duration)
- Easing: cubic-bezier(0.25, 1, 0.5, 1)

## Acrylic Backdrop

Window-level acrylic blur via DWM API. Requires:
- `WindowBuilder::with_transparent(true)`
- `Config::with_background_color((0,0,0,0))`
- `DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, 3)` (Acrylic)
- CSS body background: transparent

## Components

### Card
- Semi-transparent background over acrylic: `var(--bg-surface)`
- 2px border, 6px radius
- 16px padding
- Title in DM Mono 17px weight 500
- Hover: hard offset shadow

### Buttons
- 2px border, 4px radius, 8px 16px padding
- Primary: accent fill, white text, hard shadow
- Primary active: shadow removed, translate(1px, 2px) pressed effect

### Select / Input
- 2px border, 4px radius, 6px 12px padding
- Focus: border-color: var(--accent)
- Hover: border-color: var(--border-strong)

### Toggle
- 2px border, pill shape
- Active: accent fill

### Tag Chips
- 2px border, 4px radius
- Monospace font, tabular-nums

## Custom Title Bar

- 32px height, DM Mono title
- 2px border-bottom
- Close hover: danger red

## Overlay Window

- Dark glass (unchanged from previous design)
- Monospace text, 8px radius

## Screen Edge Glow

- Purple glow: #4B0082 (unchanged)
- Configurable via settings
```

**Step 2: Update CLAUDE.md design language reference**

In `CLAUDE.md`, change the design language line from:
```
- Design language: Developer Precision (monochrome + purple accent, borders-only, Mica backdrop)
```
to:
```
- Design language: Deploy Purple (purple accent, 2px borders, hard-offset shadows, acrylic backdrop)
```

**Step 3: Commit**

```bash
git add agent_docs/design_system.md CLAUDE.md
git commit -m "docs: update design system documentation for Deploy Purple"
```

---

### Task 5: Visual verification and polish

**Files:**
- Possibly modify: `assets/styles.css` (minor tweaks)

**Step 1: Run the app and check each page**

Run: `cargo run`

Check each page visually:
1. **Home page**: Status card, Recent card, Quick Settings card — all should have 2px borders, DM Mono titles, hard shadows on hover
2. **History page**: Date headers in DM Mono uppercase, entries with hover backgrounds
3. **Settings page**: All 6 cards (Recording, Transcription, API Keys, Vocabulary, Appearance, Debug) — verify borders, inputs, toggles, radio buttons, tag chips, status log
4. **Save button**: Should have accent fill + hard shadow, pressed effect on click
5. **Sidebar**: 2px border-right, active icon has accent-subtle background
6. **Title bar**: DM Mono "Beamer", 2px border-bottom
7. **Acrylic**: Window should show blur-through behind semi-transparent surfaces

**Step 2: Fix any visual issues found**

If fonts didn't load: check the `@font-face` `src` paths — they should be relative from the CSS file location (`fonts/DMMono-Regular.woff2`). If Dioxus asset serving differs, may need `url("/assets/fonts/...")`.

If acrylic doesn't work: Windows 11 22H2+ required. On older versions, fallback is opaque white surfaces (still looks fine).

**Step 3: Final commit if any polish was needed**

```bash
git add -A
git commit -m "fix: polish Deploy Purple visual details"
```
