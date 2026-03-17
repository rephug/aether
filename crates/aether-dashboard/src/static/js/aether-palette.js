/**
 * AETHER Unified Color Palette
 *
 * Single source of truth for all chart colors, status indicators,
 * and accent colors across the dashboard. All D3 chart modules
 * should reference window.AetherPalette instead of hardcoded hex values.
 */
(function () {
  var dark = function () {
    return document.documentElement.classList.contains('dark');
  };

  var PALETTE = {
    /* ── Categorical (for D3 chart series) ──────────────── */
    categorical: [
      '#0ea5a8', /* cyan    — primary */
      '#ea7d28', /* orange  — secondary */
      '#5e6db3', /* purple  — tertiary */
      '#1b9a59', /* green   */
      '#b7871d', /* yellow  */
      '#c94a34', /* red     */
      '#64748b', /* slate   */
      '#8b5cf6', /* violet  */
    ],

    /* ── Status ─────────────────────────────────────────── */
    ok:     '#10b981',
    warn:   '#f59e0b',
    danger: '#ef4444',
    info:   '#0ea5a8',

    /* ── Named accents (match Tailwind config) ──────────── */
    cyan:   '#0ea5a8',
    orange: '#ea7d28',
    green:  '#1b9a59',
    red:    '#c94a34',
    purple: '#5e6db3',
    yellow: '#b7871d',

    /* ── Text & borders (adaptive to theme) ─────────────── */
    text:       function () { return dark() ? '#94a3b8' : '#64748b'; },
    textStrong: function () { return dark() ? '#e2e8f0' : '#1e293b'; },
    border:     function () { return dark() ? '#334155' : '#cbd5e1'; },
    gridLine:   function () { return dark() ? 'rgba(51,65,85,0.5)' : 'rgba(203,213,225,0.5)'; },

    /* ── Chart backgrounds ──────────────────────────────── */
    chartBg:    function () { return dark() ? 'rgba(15,23,42,0.75)' : '#10151c'; },

    /* ── D3 tooltip (matches AetherTooltip) ─────────────── */
    tooltipBg:  function () { return dark() ? 'rgba(250,250,250,0.96)' : 'rgba(15,23,42,0.95)'; },
    tooltipText: function () { return dark() ? '#0f172a' : '#f8fafc'; },
  };

  window.AetherPalette = PALETTE;
})();
