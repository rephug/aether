(function () {
  function empty(el, msg) {
    el.innerHTML = `<div class="chart-empty"><div class="empty-state"><div class="empty-state-title">${msg}</div></div></div>`;
  }

  const STATUS_COLORS = {
    good: '#10b981',
    warn: '#f59e0b',
    critical: '#ef4444',
  };

  function drawSparkline(container, trend, color) {
    if (!trend || !trend.length) return;
    container.innerHTML = '';
    const w = container.clientWidth || 120;
    const h = 40;
    const m = { top: 4, right: 4, bottom: 4, left: 4 };
    const x = d3.scaleLinear().domain([0, trend.length - 1]).range([m.left, w - m.right]);
    const yMax = d3.max(trend) || 1;
    const y = d3.scaleLinear().domain([0, yMax * 1.1]).range([h - m.bottom, m.top]);
    const area = d3.area().x((_d, i) => x(i)).y0(h - m.bottom).y1((d) => y(d));
    const line = d3.line().x((_d, i) => x(i)).y((d) => y(d));
    const svg = d3.select(container).append('svg').attr('width', w).attr('height', h);

    const gradId = `sc-grad-${Math.random().toString(36).slice(2)}`;
    const defs = svg.append('defs');
    const grad = defs.append('linearGradient').attr('id', gradId).attr('x1', '0%').attr('x2', '0%').attr('y1', '0%').attr('y2', '100%');
    grad.append('stop').attr('offset', '0%').attr('stop-color', color).attr('stop-opacity', 0.3);
    grad.append('stop').attr('offset', '100%').attr('stop-color', color).attr('stop-opacity', 0.03);

    svg.append('path').datum(trend).attr('fill', `url(#${gradId})`).attr('d', area);
    svg.append('path').datum(trend).attr('fill', 'none').attr('stroke', color).attr('stroke-width', 2).attr('d', line);
    svg.append('circle')
      .attr('cx', x(trend.length - 1)).attr('cy', y(trend[trend.length - 1]))
      .attr('r', 3).attr('fill', color);
  }

  function renderCards(container, metrics) {
    container.innerHTML = '';
    if (!metrics || !metrics.length) { empty(container, 'No health metrics available'); return; }

    metrics.forEach((m) => {
      const color = STATUS_COLORS[m.status] || '#94a3b8';
      const card = document.createElement('div');
      card.className = 'stat-card rounded-lg border p-3 space-y-1';
      card.style.borderColor = color;

      const displayValue = typeof m.value === 'number'
        ? (Number.isInteger(m.value) ? `${m.value}` : m.value.toFixed(2))
        : `${m.value}`;

      card.innerHTML =
        '<div class="flex items-center justify-between">' +
        `<span class="text-xs text-text-secondary font-medium">${m.label || m.name}</span>` +
        `<span class="text-xs px-1.5 py-0.5 rounded font-semibold" style="background:${color}20;color:${color}">${m.status}</span>` +
        '</div>' +
        `<div class="text-2xl font-bold text-text-primary">${displayValue}` +
        `<span class="text-xs text-text-secondary ml-1">${m.unit || ''}</span>` +
        '</div>' +
        '<div class="sparkline-container" style="width:100%;height:40px"></div>';

      container.appendChild(card);
      const sparkContainer = card.querySelector('.sparkline-container');
      if (sparkContainer && m.trend && m.trend.length > 1) {
        drawSparkline(sparkContainer, m.trend, color);
      }
    });
  }

  window.initHealthScorecard = function initHealthScorecard() {
    const container = document.getElementById('health-scorecard-grid');
    if (!container) return;

    fetch('/api/v1/health-scorecard', { headers: { Accept: 'application/json' } })
      .then((r) => r.json())
      .then((json) => renderCards(container, (json.data || json).metrics))
      .catch(() => empty(container, 'Failed to load health scorecard'));
  };
})();
