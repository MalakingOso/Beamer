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
