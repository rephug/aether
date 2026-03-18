(function () {
  const tip = () => window.AetherTooltip || { show() {}, hide() {} };

  function empty(el, msg) {
    el.innerHTML = `<div class="chart-empty"><div class="empty-state"><div class="empty-state-title">${msg}</div></div></div>`;
  }

  function stabilityColor(value) {
    // Green (stable) → Yellow (shifting) → Red (fault line)
    return d3.interpolateRdYlGn(value);
  }

  function stabilityLabel(value) {
    if (value >= 0.8) return 'Bedrock';
    if (value >= 0.5) return 'Shifting';
    return 'Fault Line';
  }

  function draw(container, data) {
    container.innerHTML = '';
    const communities = data.communities || [];
    if (!communities.length) { empty(container, 'No community stability data yet. Run the seismograph engine to compute stability.'); return; }

    const P = window.AetherPalette || {};
    const textColor = typeof P.text === 'function' ? P.text() : '#64748b';

    const width = container.clientWidth || 800;
    const height = Math.max(400, Math.min(600, communities.length * 40));

    // Build hierarchy for treemap
    const root = d3.hierarchy({
      name: 'root',
      children: communities.map((c) => ({
        name: c.community_id,
        value: Math.max(c.symbol_count, 1),
        stability: c.stability,
        symbol_count: c.symbol_count,
        breach_count: c.breach_count,
      })),
    }).sum((d) => d.value);

    d3.treemap()
      .size([width, height])
      .padding(3)
      .round(true)(root);

    const svg = d3.select(container).append('svg')
      .attr('width', width)
      .attr('height', height);

    const leaves = svg.selectAll('g')
      .data(root.leaves())
      .join('g')
      .attr('transform', (d) => `translate(${d.x0},${d.y0})`);

    // Rectangles
    leaves.append('rect')
      .attr('width', (d) => d.x1 - d.x0)
      .attr('height', (d) => d.y1 - d.y0)
      .attr('fill', (d) => stabilityColor(d.data.stability))
      .attr('stroke', '#fff')
      .attr('stroke-width', 1.5)
      .attr('rx', 4)
      .style('cursor', 'pointer')
      .on('mouseover', (event, d) => {
        const label = stabilityLabel(d.data.stability);
        tip().show(event,
          `<strong>Community ${d.data.name}</strong><br/>` +
          `Stability: ${d.data.stability.toFixed(3)} (${label})<br/>` +
          `Symbols: ${d.data.symbol_count}<br/>` +
          `Breaches: ${d.data.breach_count}`
        );
      })
      .on('mouseout', () => tip().hide());

    // Labels (only if tile is large enough)
    leaves.each(function (d) {
      const tileW = d.x1 - d.x0;
      const tileH = d.y1 - d.y0;
      if (tileW < 50 || tileH < 30) return;

      const g = d3.select(this);
      const labelColor = d.data.stability > 0.6 ? '#1e293b' : '#ffffff';

      g.append('text')
        .attr('x', 6)
        .attr('y', 16)
        .attr('fill', labelColor)
        .attr('font-size', 11)
        .attr('font-weight', 600)
        .text('C' + d.data.name);

      if (tileH >= 44) {
        g.append('text')
          .attr('x', 6)
          .attr('y', 30)
          .attr('fill', labelColor)
          .attr('font-size', 9)
          .attr('opacity', 0.8)
          .text(d.data.stability.toFixed(2) + ' \u00b7 ' + d.data.symbol_count + ' sym');
      }
    });
  }

  window.initSeismographPlates = function initSeismographPlates() {
    const container = document.getElementById('seismograph-plates-chart');
    if (!container) return;

    fetch('/api/v1/seismograph-plates', { headers: { Accept: 'application/json' } })
      .then((r) => r.json())
      .then((json) => draw(container, json.data || json))
      .catch(() => empty(container, 'Failed to load tectonic plates data'));
  };
})();
