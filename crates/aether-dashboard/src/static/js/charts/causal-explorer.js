(function () {
  function render(data) {
    const container = document.getElementById('causal-graph');
    if (!container) return;
    container.innerHTML = '';

    if (!data || !data.target) {
      container.innerHTML = '<div class="chart-empty"><div class="empty-state-title">Select a target symbol</div></div>';
      return;
    }

    const width = container.clientWidth || 980;
    const layerW = 220;
    const rowH = 96;

    const rows = [
      {
        symbol_id: data.target.symbol_id,
        qualified_name: data.target.qualified_name,
        timestamp: Date.now(),
        drift_score: 0,
        sir_diff_summary: 'Target symbol',
        causal_confidence: data.overall_confidence || 0.5,
        link_type: 'target',
        caused: [],
      },
      ...(data.chain || []),
    ];

    rows.sort((a, b) => Number(a.timestamp || 0) - Number(b.timestamp || 0));

    const height = Math.max(560, rows.length * rowH + 40);
    const svg = d3.select(container).append('svg').attr('width', width).attr('height', height);

    const tip = window.AetherTooltip;

    rows.forEach((row, i) => {
      row.layer = i;
      row.x = 20 + i * layerW;
      row.y = 30 + (i % Math.max(1, Math.floor((height - 50) / rowH))) * rowH;
    });

    const link = d3.linkHorizontal().x((d) => d.x).y((d) => d.y);
    const links = [];
    for (let i = 1; i < rows.length; i++) {
      links.push({
        source: { x: rows[i - 1].x + 170, y: rows[i - 1].y + 26 },
        target: { x: rows[i].x, y: rows[i].y + 26 },
        row: rows[i],
      });
    }

    svg.selectAll('path.link')
      .data(links)
      .join('path')
      .attr('d', link)
      .attr('fill', 'none')
      .attr('stroke', (d) => d.row.link_type === 'dependency' ? '#0ea5a8' : '#f59e0b')
      .attr('stroke-width', (d) => 1 + 3 * Number(d.row.causal_confidence || 0))
      .attr('stroke-dasharray', (d) => d.row.link_type === 'co_change' ? '6,4' : '0');

    const nodes = svg.selectAll('g.node')
      .data(rows)
      .join('g')
      .attr('transform', (d) => `translate(${d.x},${d.y})`)
      .style('cursor', 'pointer')
      .on('mouseover', (event, d) => {
        tip.show(event, `<strong>${d.qualified_name}</strong><br/>${d.sir_diff_summary}`);
      })
      .on('mouseout', () => tip.hide())
      .on('click', (_event, d) => {
        if (!d.symbol_id) return;
        const hidden = document.getElementById('causal-symbol-id');
        const input = document.getElementById('causal-symbol-input');
        if (hidden) hidden.value = d.symbol_id;
        if (input) input.value = d.qualified_name;
        load();
      });

    nodes.append('rect')
      .attr('width', 170)
      .attr('height', 54)
      .attr('rx', 8)
      .attr('fill', '#ffffff')
      .attr('stroke', (d) => {
        const c = Number(d.causal_confidence || 0);
        if (c >= 0.7) return '#10b981';
        if (c >= 0.4) return '#f59e0b';
        return '#94a3b8';
      })
      .attr('stroke-width', (d, i) => i === 0 ? 2.4 : 1.2)
      .attr('filter', (d, i) => i === 0 ? 'drop-shadow(0 0 8px rgba(14,165,168,0.25))' : null);

    nodes.append('text').attr('x', 8).attr('y', 16).attr('font-size', 11).attr('fill', '#0f172a').text((d) => d.qualified_name.split('::').pop());
    nodes.append('text').attr('x', 8).attr('y', 30).attr('font-size', 10).attr('fill', '#475569').text((d) => `drift ${Number(d.drift_score || 0).toFixed(2)} · conf ${Number(d.causal_confidence || 0).toFixed(2)}`);
    nodes.append('text').attr('x', 8).attr('y', 44).attr('font-size', 9).attr('fill', '#64748b').text((d) => (d.sir_diff_summary || 'No SIR diff').slice(0, 45));
  }

  function load() {
    const id = document.getElementById('causal-symbol-id')?.value?.trim();
    if (!id) {
      render(null);
      return;
    }
    const depth = Number(document.getElementById('causal-depth')?.value || 3);
    const lookback = document.getElementById('causal-lookback')?.value || '30d';

    fetch(`/api/v1/causal-chain?symbol_id=${encodeURIComponent(id)}&depth=${depth}&lookback=${encodeURIComponent(lookback)}`)
      .then((r) => r.json())
      .then((json) => {
        window.__AETHER_CAUSAL = json?.data || null;
        render(window.__AETHER_CAUSAL);
      })
      .catch(() => render(null));
  }

  window.initCausalExplorer = function initCausalExplorer() {
    const page = document.querySelector('[data-page="causal"]');
    if (!page) return;

    if (window.initSymbolSearchComponents) window.initSymbolSearchComponents();

    page.addEventListener('aether:symbol-selected', (event) => {
      const hidden = document.getElementById('causal-symbol-id');
      if (hidden) hidden.value = event.detail.symbol_id;
      load();
    });

    const depth = document.getElementById('causal-depth');
    const lookback = document.getElementById('causal-lookback');
    if (depth && !depth.dataset.bound) {
      depth.dataset.bound = '1';
      depth.addEventListener('change', load);
    }
    if (lookback && !lookback.dataset.bound) {
      lookback.dataset.bound = '1';
      lookback.addEventListener('change', load);
    }

    const animate = document.getElementById('causal-animate');
    if (animate && !animate.dataset.bound) {
      animate.dataset.bound = '1';
      animate.addEventListener('click', () => {
        const svg = document.querySelector('#causal-graph svg');
        if (!svg) return;
        const nodes = d3.select(svg).selectAll('g.node');
        nodes.style('opacity', 0.2);
        nodes.each(function (_d, i) {
          d3.select(this).transition().delay(i * 800).duration(350).style('opacity', 1);
        });
      });
    }

    load();
  };
})();
