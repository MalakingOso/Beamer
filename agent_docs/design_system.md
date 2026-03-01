# Design System — Developer Precision

## Philosophy

Monochrome + purple accent. Borders-only depth (no shadows). Mica/Acrylic backdrop. Every pixel intentional.

## Color Tokens

```css
:root {
  --bg: transparent;
  --bg-card: rgba(255,255,255,0.6);
  --accent: #4B0082;
  --accent-hover: #5C1A9E;
  --fg: #000000;
  --fg-secondary: rgba(0,0,0,0.7);
  --fg-muted: rgba(0,0,0,0.5);
  --fg-faint: rgba(0,0,0,0.3);
  --border: rgba(0,0,0,0.08);
  --border-active: rgba(75,0,130,0.3);
  --danger: #DC2626;
  --success: #16A34A;
}
```

## Typography

Three layers:
1. **Display/Headers:** `Geist` — clean, technical feel
2. **UI Chrome:** `Segoe UI Variable` — Windows native, invisible
3. **Data/Debug:** `Cascadia Code` / `JetBrains Mono` — monospace

## Spacing

4px base grid:
- 8px — within components
- 12px — standard gap
- 16px — section padding
- 24px — between cards

## Border Radius

Sharp system: 4px cards, 4px inputs, 6px buttons.

## Depth

Borders only. No box shadows. Cards: `border: 0.5px solid var(--border)`.
Active/focused: `border-color: var(--border-active)`.

## Components

### Card
- Semi-transparent background over Mica: `var(--bg-card)`
- 0.5px border, 4px radius
- 16px padding
- Section title in header

### Select (custom dropdown)
- Not native `<select>` — custom styled
- Border-only, 4px radius
- Dropdown appears below with same card styling
- Selected item shown with accent color

### Toggle
- Pill shape, 20px height
- Off: border-only, transparent
- On: filled with `var(--accent)`

### MaskedInput
- For API keys — shows `••••••••` by default
- [Show] button toggles visibility
- Monospace font when visible

### TagChip
- For vocabulary terms
- Pill shape, border-only
- `✕` button to remove
- Monospace font for term text

## Custom Title Bar

- 32px height
- Left: "Beamer" in Geist font
- Right: minimize (─) and close (✕) buttons
- Close hover: `var(--danger)` background
- Bottom border: `var(--border)`
- Entire bar is drag region (except buttons)

## Overlay Window

- 300x80px, bottom-center of screen
- Semi-transparent dark background: `rgba(0,0,0,0.75)`
- White monospace text
- 8px border radius
- No border

## Screen Edge Glow

- Fullscreen transparent window
- CSS: `box-shadow: inset 0 0 8px 3px var(--glow-color)`
- Default glow color: `#4B0082` (purple)
- Configurable via appearance settings
- Click-through (doesn't intercept mouse events)
