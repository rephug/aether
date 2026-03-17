# AETHER Dashboard Polish

## When to use

When improving the visual design of AETHER's HTMX + D3 + Tailwind dashboard. This skill guides production-grade UI polish that avoids generic AI aesthetics.

## Existing aesthetic

The dashboard has an established identity — don't reinvent it, refine it:

- **Font:** IBM Plex Sans (already loaded via Google Fonts — bundle locally for Tauri)
- **Palette:** Earthy/natural surfaces (surface-0 `#f7f7f4` through surface-4 `#c7ccb9`) with accent-cyan `#0ea5a8` as primary action color
- **Dark mode:** Slate-based (`slate-900`, `slate-950`) — already implemented, needs contrast audit
- **Tone:** Technical but approachable. A developer intelligence tool, not a consumer app. Think: clean IDE aesthetic meets data dashboard

## Design principles for AETHER

- **Information density over whitespace.** Users are developers and analysts — they want data visible, not hidden behind clicks. Prefer compact cards over sparse layouts.
- **Consistency is the polish.** The biggest win is making every page use the same card, table, metric, and badge components rather than inventing new ones.
- **D3 charts are the hero.** The visualizations are AETHER's differentiator. Charts should feel premium — consistent color palette, smooth transitions, proper axis labeling, responsive sizing.
- **Dark mode is primary.** Most developers use dark mode. Design dark-first, verify light second.
- **Offline-first.** Bundle all CDN dependencies locally (Tailwind, D3, HTMX, Tippy, IBM Plex Sans). The Tauri desktop app must work without internet.

## Component standards

### Cards
```css
.aether-card {
  @apply rounded-lg border border-surface-3/50 bg-surface-1/85 p-4 
         dark:bg-slate-800/80 dark:border-slate-700
         hover:shadow-sm transition-shadow;
}
```

### Metric display (large number + sparkline + badge)
Standardize the pattern already used in health scoring across all operational pages.

### Tables
Striped rows, sticky headers, hover highlight. Monospace for symbol names and file paths.

### Status badges
- Green (`accent-green`): healthy, fresh, complete
- Yellow/amber (`accent-orange`): warning, aging, partial
- Red (`accent-red`): error, stale, critical
- Cyan (`accent-cyan`): active, in-progress, primary action

### Empty states
SVG illustration + message + suggested action. Not just "No data available."

## D3 chart standards

### Unified color scale
All charts should pull from a shared palette defined in `charts.js`:
- Sequential: cyan → emerald gradient for positive metrics
- Diverging: red → yellow → green for health/staleness
- Categorical: 8 distinct colors for multi-series charts, consistent across modules

### Tooltips
Use the existing Tippy.js integration. Consistent dark background, rounded corners, 12px font, max-width 320px.

### Loading states
Skeleton placeholder (subtle pulse animation) while data fetches and D3 renders. Not a spinner, not a blank div.

### Responsive
All charts must handle window resize via ResizeObserver. Critical for Tauri where users resize freely.

## What NOT to do

- Don't change the font from IBM Plex Sans
- Don't introduce a JS framework (React/Vue/Svelte) — stay HTMX
- Don't add heavy animation libraries — CSS transitions only
- Don't change the earthy/slate color palette — refine it
- Don't modify Rust API routes or data models — CSS/HTML/JS only
- Don't use generic AI design patterns (purple gradients, oversized rounded corners, excessive whitespace)
