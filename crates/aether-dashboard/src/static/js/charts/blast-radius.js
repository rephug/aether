(function () {
  function parseParams() {
    const hidden = document.getElementById('blast-symbol-id');
    const depth = document.getElementById('blast-depth');
    const min = document.getElementById('blast-min-coupling');
    return {
      symbol_id: hidden ? hidden.value.trim() : '',
      depth: depth ? Number(depth.value || 3) : 3,
      min_coupling: min ? Number(min.value || 0.2) : 0.2,
    };
  }

  function draw(data) {
    const container = document.getElementById('blast-radius-chart');
    if (!container) return;
    container.innerHTML = '';

    if (!data || !data.center) {
      container.innerHTML = '<div class="chart-empty"><div class="empty-state-title">Select a symbol to view blast radius</div></div>';
      return;
    }

    const width = container.clientWidth || 900;
    const height = Math.max(560, container.clientHeight || 560);
    const svg = d3.select(container).append('svg').attr('width', width).attr('height', height);
    const g = svg.append('g');
    const center = { x: width / 2, y: height / 2 };

    const rings = data.rings || [];
    const maxHop = d3.max(rings, (r) => r.hop) || 1;
    const ringGap = Math.min(width, height) / (maxHop * 2 + 2);

    for (let hop = 1; hop <= maxHop; hop++) {
      g.append('circle')
        .attr('cx', center.x)
        .attr('cy', center.y)
        .attr('r', hop * ringGap)
        .attr('fill', 'none')
        .attr('stroke', '#94a3b8')
        .attr('stroke-opacity', 0.3)
        .attr('stroke-dasharray', '4,4');
    }

    const nodes = [data.center];
    (data.rings || []).forEach((ring) => {
      const total = Math.max(1, ring.nodes.length);
      ring.nodes.forEach((n, i) => {
        const angle = (Math.PI * 2 * i) / total;
        const r = ring.hop * ringGap;
        n.x = center.x + Math.cos(angle) * r;
        n.y = center.y + Math.sin(angle) * r;
        n.hop = ring.hop;
        nodes.push(n);
      });
    });

    data.center.x = center.x;
    data.center.y = center.y;
    data.center.hop = 0;

    const byId = new Map(nodes.map((n) => [n.symbol_id, n]));
    const links = [];
    nodes.forEach((n) => {
      if (!n.parent_symbol_id) return;
      if (byId.has(n.parent_symbol_id)) {
        links.push({ source: byId.get(n.parent_symbol_id), target: n, coupling: n.coupling_to_parent });
      }
    });

    g.selectAll('line.link')
      .data(links)
      .join('line')
      .attr('x1', (d) => d.source.x)
      .attr('y1', (d) => d.source.y)
      .attr('x2', (d) => d.target.x)
      .attr('y2', (d) => d.target.y)
      .attr('stroke', (d) => {
        const t = (d.coupling?.type || '').toLowerCase();
        if (t.includes('semantic')) return '#22c55e';
        if (t.includes('temporal')) return '#f59e0b';
        return '#3b82f6';
      })
      .attr('stroke-opacity', 0.6)
      .attr('stroke-width', (d) => 1 + 3 * Number(d.coupling?.strength || 0))
      .attr('stroke-dasharray', (d) => {
        const t = (d.coupling?.type || '').toLowerCase();
        if (t.includes('semantic')) return '6,3';
        if (t.includes('temporal')) return '2,3';
        return '0';
      });

    const prScale = d3.scaleSqrt().domain([0, d3.max(nodes, (n) => n.pagerank || 0) || 1]).range([5, 15]);
    const tip = window.AetherTooltip;

    const nodeSel = g.selectAll('circle.node')
      .data(nodes)
      .join('circle')
      .attr('cx', (d) => d.x)
      .attr('cy', (d) => d.y)
      .attr('r', (d) => prScale(d.pagerank || 0))
      .attr('fill', (d) => window.AetherTheme.riskColor(d.risk_score || 0))
      .attr('stroke', (d) => d.has_tests ? '#0f172a' : '#f59e0b')
      .attr('stroke-width', 1.5)
      .attr('stroke-dasharray', (d) => d.has_tests ? '0' : '4,3')
      .style('cursor', 'pointer')
      .on('mouseover', (event, d) => {
        tip.show(event, `<strong>${d.qualified_name}</strong><br/>risk ${Number(d.risk_score || 0).toFixed(2)} / importance ${Number(d.pagerank || 0).toFixed(3)}<br/>${d.file_path}`);
      })
      .on('mouseout', () => tip.hide())
      .on('click', (event, d) => {
        if (event.shiftKey) {
          const encoded = encodeURIComponent(d.symbol_id);
          htmx.ajax('GET', `/dashboard/frag/symbol/${encoded}`, {
            target: '#main-content',
            pushURL: `/dashboard/symbol/${encoded}`,
          });
          return;
        }
        const hidden = document.getElementById('blast-symbol-id');
        const input = document.getElementById('blast-symbol-input');
        if (hidden) hidden.value = d.symbol_id;
        if (input) input.value = d.qualified_name;
        load();
      });

    if (window.AetherAnimate) window.AetherAnimate.enterTransition(nodeSel);

    svg.call(d3.zoom().scaleExtent([0.4, 4]).on('zoom', (event) => g.attr('transform', event.transform)));
  }

  function load() {
    const p = parseParams();
    if (!p.symbol_id) {
      draw(null);
      return;
    }

    fetch(`/api/v1/blast-radius?symbol_id=${encodeURIComponent(p.symbol_id)}&depth=${p.depth}&min_coupling=${p.min_coupling}`)
      .then((r) => {
        if (r.status === 404) return null;
        return r.json();
      })
      .then((json) => draw(json?.data || null))
      .catch(() => draw(null));
  }

  window.initBlastRadius = function initBlastRadius() {
    const page = document.querySelector('[data-page="blast-radius"]');
    if (!page) return;

    if (window.initSymbolSearchComponents) window.initSymbolSearchComponents();

    page.addEventListener('aether:symbol-selected', (event) => {
      const hidden = document.getElementById('blast-symbol-id');
      if (hidden) hidden.value = event.detail.symbol_id;
      load();
    });

    const depth = document.getElementById('blast-depth');
    const min = document.getElementById('blast-min-coupling');
    if (depth) depth.addEventListener('change', load);
    if (min) min.addEventListener('change', load);

    load();
  };
})();
