# Design System — Beamer Purple

## Philosophy

Solid, hard-edged UI with structural borders and purple accent. Monospace display type, hard-offset shadows, no blur effects. Single accent (#4B0082). Micro-motion only.

## Color Tokens

```css
:root {
  --bg: #f5f5f7;          /* Page background */
  --bg-surface: #ffffff;  /* Cards, panels, inputs */
  --bg-hover: #f1f3f9;    /* Interactive hover */
  --bg-active: #e8ebf4;   /* Pressed/active */
  --bg-recessed: #f1f3f9; /* Sunken areas */

  --border: rgba(75, 0, 130, 0.12);        /* 2px solid everywhere */
  --border-strong: rgba(75, 0, 130, 0.25); /* Focus rings, emphasis */

  --fg: #0f152a;          /* Primary text */
  --fg-secondary: #64708b; /* Body, descriptions */
  --fg-muted: #94a0b8;    /* Labels, metadata */
  --fg-faint: #b8c0d4;    /* Placeholders, disabled */

  --accent: #4B0082;
  --accent-hover: #5C1A9E;
  --accent-subtle: rgba(75, 0, 130, 0.08);
  --accent-wash: rgba(75, 0, 130, 0.04);

  --danger: #DC2626;
  --danger-subtle: rgba(220, 38, 38, 0.06);
  --success: #16A34A;
}
```

## Typography

Three layers (fonts loaded via `ui::fonts::embedded_font_css` and injected at window creation):
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

## Components

### Card
- Solid white background: `var(--bg-surface)`
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
