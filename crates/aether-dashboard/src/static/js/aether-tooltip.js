(function () {
  let tooltip = null;

  function ensureTooltip() {
    if (tooltip) return tooltip;
    tooltip = document.createElement('div');
    tooltip.id = 'aether-tooltip';
    tooltip.style.position = 'absolute';
    tooltip.style.pointerEvents = 'none';
    tooltip.style.zIndex = '90';
    tooltip.style.maxWidth = '340px';
    tooltip.style.padding = '8px 10px';
    tooltip.style.borderRadius = '8px';
    tooltip.style.fontSize = '12px';
    tooltip.style.opacity = '0';
    tooltip.style.transition = 'opacity 120ms ease';
    document.body.appendChild(tooltip);
    return tooltip;
  }

  function show(event, htmlContent) {
    const el = ensureTooltip();
    const dark = window.AetherTheme && window.AetherTheme.isDark && window.AetherTheme.isDark();
    el.style.background = dark ? 'rgba(250,250,250,0.96)' : 'rgba(15,23,42,0.95)';
    el.style.color = dark ? '#0f172a' : '#f8fafc';
    el.style.border = dark ? '1px solid rgba(15,23,42,0.2)' : '1px solid rgba(148,163,184,0.2)';
    el.innerHTML = htmlContent;

    const scrollX = window.scrollX || document.documentElement.scrollLeft;
    const scrollY = window.scrollY || document.documentElement.scrollTop;
    let x = event.clientX + 14 + scrollX;
    let y = event.clientY + 12 + scrollY;

    const rect = el.getBoundingClientRect();
    const vw = window.innerWidth;
    const vh = window.innerHeight;

    if (x + rect.width > scrollX + vw - 8) x = scrollX + vw - rect.width - 8;
    if (y + rect.height > scrollY + vh - 8) y = scrollY + vh - rect.height - 8;
    if (x < scrollX + 8) x = scrollX + 8;
    if (y < scrollY + 8) y = scrollY + 8;

    el.style.left = `${x}px`;
    el.style.top = `${y}px`;
    el.style.opacity = '1';
  }

  function hide() {
    if (!tooltip) return;
    tooltip.style.opacity = '0';
  }

  window.AetherTooltip = { show, hide };
})();
