(function () {
  function formatValue(v) {
    if (v == null) return '—';
    if (typeof v === 'number') return Number.isInteger(v) ? `${v}` : v.toFixed(2);
    return `${v}`;
  }

  function drawSparkline(container, points, color) {
    const data = Array.isArray(points) ? points : [];
    if (!data.length) {
      container.innerHTML = '';
      return;
    }

    const w = 90;
    const h = 24;
    const m = { top: 2, right: 2, bottom: 2, left: 2 };
    const x = d3.scaleLinear().domain([0, data.length - 1]).range([m.left, w - m.right]);
    const y = d3.scaleLinear().domain([0, d3.max(data) || 1]).nice().range([h - m.bottom, m.top]);
    const area = d3.area().x((_d, i) => x(i)).y0(h - m.bottom).y1((d) => y(d));
    const line = d3.line().x((_d, i) => x(i)).y((d) => y(d));

    container.innerHTML = '';
    const svg = d3.select(container).append('svg').attr('width', w).attr('height', h);
    svg.append('path').datum(data).attr('fill', d3.color(color).copy({ opacity: 0.16 })).attr('d', area);
    svg.append('path').datum(data).attr('fill', 'none').attr('stroke', color).attr('stroke-width', 1.5).attr('d', line);
  }

  window.initXrayCards = function initXrayCards() {
    const page = document.querySelector('[data-page="xray"]');
    if (!page) return;

    const windowVal = page.getAttribute('data-window') || '7d';
    fetch(`/api/v1/xray?window=${windowVal}`)
      .then((r) => r.json())
      .then((json) => {
        const metrics = json?.data?.metrics || {};
        window.__AETHER_XRAY = json?.data || null;

        Object.entries(metrics).forEach(([name, metric]) => {
          const valueEl = document.getElementById(`metric-value-${name}`);
          const trendEl = document.getElementById(`metric-trend-${name}`);
          const sparkEl = document.getElementById(`sparkline-${name}`);
          const card = document.getElementById(`metric-card-${name}`);
          if (!valueEl || !trendEl || !sparkEl || !card) return;

          valueEl.textContent = formatValue(metric?.value);
          const trend = Number(metric?.trend || 0);
          trendEl.textContent = trend > 0 ? `↑ ${trend.toFixed(2)}` : trend < 0 ? `↓ ${Math.abs(trend).toFixed(2)}` : '─';

          let statusValue = Number(metric?.value);
          if (!Number.isFinite(statusValue)) {
            statusValue = name === 'risk_grade' ? ({ 'A+': 0.95, A: 0.9, 'B+': 0.8, B: 0.72, C: 0.58, D: 0.42, F: 0.1 }[String(metric?.value)] || 0.5) : 0.5;
          }
          const color = window.AetherTheme ? window.AetherTheme.statusColor(statusValue, { good: 0.75, warn: 0.45 }) : '#0ea5a8';
          card.style.borderColor = color;
          drawSparkline(sparkEl, metric?.sparkline, color);
        });
      })
      .catch(() => {
        // keep placeholders
      });
  };
})();
