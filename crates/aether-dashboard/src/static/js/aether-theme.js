(function () {
  function isDark() {
    return document.documentElement.classList.contains('dark');
  }

  function applyTheme(mode) {
    if (mode === 'dark') {
      document.documentElement.classList.add('dark');
      localStorage.theme = 'dark';
    } else {
      document.documentElement.classList.remove('dark');
      localStorage.theme = 'light';
    }
  }

  function toggleTheme() {
    applyTheme(isDark() ? 'light' : 'dark');
  }

  function statusColor(value, thresholds) {
    const v = Number(value ?? 0);
    const good = thresholds?.good ?? 0.8;
    const warn = thresholds?.warn ?? 0.5;
    if (v >= good) return '#10b981';
    if (v >= warn) return '#f59e0b';
    return '#ef4444';
  }

  function riskColor(score) {
    const s = Math.max(0, Math.min(1, Number(score ?? 0)));
    return d3.interpolateRgb('#10b981', '#ef4444')(s);
  }

  function communityColor(index) {
    const idx = Math.abs(Number(index ?? 0)) % 10;
    return d3.schemeTableau10[idx];
  }

  window.AetherTheme = {
    isDark,
    applyTheme,
    toggleTheme,
    statusColor,
    riskColor,
    communityColor,
  };
})();
