(function () {
  const tip = () => window.AetherTooltip || { show() {}, hide() {} };

  function empty(el, msg) {
    el.innerHTML = `<div class="chart-empty"><div class="empty-state"><div class="empty-state-title">${msg}</div></div></div>`;
  }

  function draw(container, data) {
    container.innerHTML = '';
    const modules = data.modules || [];
    const dates = data.dates || [];
    const cells = data.cells || [];
    if (!modules.length || !dates.length) { empty(container, 'No staleness data available'); return; }

    const P = window.AetherPalette || {};
    const textColor = typeof P.text === 'function' ? P.text() : '#64748b';

    const cellSize = Math.max(16, Math.min(40, Math.floor((container.clientWidth - 160) / dates.length)));
    const margin = { top: 60, right: 20, bottom: 20, left: 140 };
    const width = margin.left + dates.length * cellSize + margin.right;
    const height = margin.top + modules.length * cellSize + margin.bottom;

    // Reversed RdYlGn: 0 (fresh) = green, 1 (stale) = red
    const colorScale = d3.scaleSequential(d3.interpolateRdYlGn).domain([1, 0]);

    const svg = d3.select(container).append('svg')
      .attr('width', Math.max(width, container.clientWidth))
      .attr('height', height);

    const g = svg.append('g').attr('transform', `translate(${margin.left},${margin.top})`);

    // Date labels (top)
    dates.forEach((dateStr, j) => {
      g.append('text')
        .attr('x', j * cellSize + cellSize / 2)
        .attr('y', -8)
        .attr('text-anchor', 'middle')
        .attr('fill', textColor)
        .attr('font-size', 9)
        .attr('transform', `rotate(-45,${j * cellSize + cellSize / 2},-8)`)
        .text(dateStr.slice(5)); // MM-DD
    });

    // Module labels (left)
    modules.forEach((mod, i) => {
      g.append('text')
        .attr('x', -6)
        .attr('y', i * cellSize + cellSize / 2)
        .attr('dy', '0.35em')
        .attr('text-anchor', 'end')
        .attr('fill', textColor)
        .attr('font-size', 10)
        .text(mod.length > 18 ? `…${mod.slice(-17)}` : mod);
    });

    // Cells
    modules.forEach((mod, i) => {
      const row = cells[i] || [];
      dates.forEach((dateStr, j) => {
        const score = row[j] != null ? row[j] : 0;
        g.append('rect')
          .attr('x', j * cellSize)
          .attr('y', i * cellSize)
          .attr('width', cellSize - 1)
          .attr('height', cellSize - 1)
          .attr('rx', 2)
          .attr('fill', colorScale(score))
          .attr('stroke', 'none')
          .style('cursor', 'pointer')
          .on('mouseover', (event) => {
            d3.select(event.currentTarget).attr('stroke', '#fff').attr('stroke-width', 2);
            tip().show(event,
              `<strong>${mod}</strong><br/>` +
              `${dateStr}<br/>` +
              `Staleness: ${score.toFixed(3)}`
            );
          })
          .on('mouseout', (event) => {
            d3.select(event.currentTarget).attr('stroke', 'none');
            tip().hide();
          });
      });
    });
  }

  function load() {
    const container = document.getElementById('staleness-heatmap-chart');
    if (!container) return;
    const sinceEl = document.getElementById('staleness-heatmap-since');
    const staleOnlyEl = document.getElementById('staleness-heatmap-stale-only');
    const since = sinceEl ? sinceEl.value : '30d';
    let url = `/api/v1/staleness-heatmap?since=${since}`;
    if (staleOnlyEl && staleOnlyEl.checked) url += '&stale_only=true';

    fetch(url, { headers: { Accept: 'application/json' } })
      .then((r) => r.json())
      .then((json) => draw(container, json.data || json))
      .catch(() => empty(container, 'Failed to load staleness heatmap'));
  }

  window.initStalenessHeatmap = function initStalenessHeatmap() {
    const container = document.getElementById('staleness-heatmap-chart');
    if (!container) return;

    const sinceEl = document.getElementById('staleness-heatmap-since');
    const staleOnlyEl = document.getElementById('staleness-heatmap-stale-only');
    if (sinceEl) sinceEl.addEventListener('change', load);
    if (staleOnlyEl) staleOnlyEl.addEventListener('change', load);

    load();
  };
})();
