(function () {
  const tip = () => window.AetherTooltip || { show() {}, hide() {} };

  function parseParams() {
    const symEl = document.getElementById('blast-symbol-id');
    const depthEl = document.getElementById('blast-depth');
    const couplingEl = document.getElementById('blast-min-coupling');
    return {
      symbol_id: symEl ? symEl.value.trim() : '',
      depth: depthEl ? parseInt(depthEl.value, 10) || 3 : 3,
      min_coupling: couplingEl ? parseFloat(couplingEl.value) || 0.2 : 0.2,
    };
  }

  function buildHierarchy(data) {
    const center = data.center;
    const nodeMap = {};
    const root = { id: center.symbol_id, data: center, children: [] };
    nodeMap[center.symbol_id] = root;

    const rings = data.rings || [];
    rings.forEach((ring) => {
      (ring.nodes || []).forEach((node) => {
        nodeMap[node.symbol_id] = { id: node.symbol_id, data: node, children: [] };
      });
    });

    rings.forEach((ring) => {
      (ring.nodes || []).forEach((node) => {
        const parentId = node.parent_symbol_id || center.symbol_id;
        const parent = nodeMap[parentId] || root;
        const child = nodeMap[node.symbol_id];
        if (child && parent) parent.children.push(child);
      });
    });

    return d3.hierarchy(root);
  }

  function draw(data) {
    const container = document.getElementById('blast-radius-chart');
    if (!container) return;
    container.innerHTML = '';

    if (!data || !data.center) {
      container.innerHTML = '<div class="chart-empty"><div class="empty-state"><div class="empty-state-title">Select a symbol to explore its blast radius</div></div></div>';
      return;
    }

    const width = container.clientWidth || 800;
    const height = Math.max(560, container.clientHeight || 560);
    const radius = Math.min(width, height) / 2 - 80;

    const svg = d3.select(container).append('svg')
      .attr('width', width)
      .attr('height', height);

    const g = svg.append('g')
      .attr('transform', `translate(${width / 2},${height / 2})`);

    svg.call(d3.zoom().scaleExtent([0.4, 4]).on('zoom', (event) => {
      g.attr('transform', `translate(${width / 2},${height / 2})` + event.transform);
    }));

    const hierarchy = buildHierarchy(data);
    const treeLayout = d3.tree()
      .size([2 * Math.PI, radius])
      .separation((a, b) => (a.parent === b.parent ? 1 : 2) / a.depth);

    treeLayout(hierarchy);

    // Staleness color scale: green (0) -> red (1)
    const stalenessColor = d3.scaleSequential(d3.interpolateRdYlGn).domain([1, 0]);

    // PageRank size scale
    const allPr = [];
    hierarchy.each((d) => { allPr.push(d.data.data ? d.data.data.pagerank || 0 : 0); });
    const prScale = d3.scaleSqrt().domain([0, d3.max(allPr) || 1]).range([4, 16]);

    // Draw links
    const linkRadial = d3.linkRadial()
      .angle((d) => d.x)
      .radius((d) => d.y);

    g.selectAll('path.link')
      .data(hierarchy.links())
      .join('path')
      .attr('class', 'link')
      .attr('fill', 'none')
      .attr('d', linkRadial)
      .attr('stroke', (d) => {
        const coupling = d.target.data.data ? d.target.data.data.coupling_to_parent : null;
        if (!coupling) return '#94a3b8';
        const type = (coupling.coupling_type || '').toLowerCase();
        if (type.includes('semantic')) return '#22c55e';
        if (type.includes('temporal')) return '#f59e0b';
        if (type.includes('struct')) return '#3b82f6';
        return '#94a3b8';
      })
      .attr('stroke-width', (d) => {
        const coupling = d.target.data.data ? d.target.data.data.coupling_to_parent : null;
        const strength = coupling && coupling.strength ? coupling.strength : 0.3;
        return 1 + 2 * strength;
      })
      .attr('stroke-opacity', 0.6);

    // Draw nodes
    const nodes = g.selectAll('g.node')
      .data(hierarchy.descendants())
      .join('g')
      .attr('class', 'node')
      .attr('transform', (d) => `translate(${d3.pointRadial(d.x, d.y)})`);

    const circles = nodes.append('circle')
      .attr('r', (d) => prScale(d.data.data ? d.data.data.pagerank || 0 : 0))
      .attr('fill', (d) => {
        const drift = d.data.data ? d.data.data.drift_score || 0 : 0;
        return stalenessColor(drift);
      })
      .attr('stroke', (d) => {
        const hasTests = d.data.data ? d.data.data.has_tests : false;
        return hasTests ? '#334155' : '#f59e0b';
      })
      .attr('stroke-width', (d) => d.depth === 0 ? 3 : 1.5)
      .attr('stroke-dasharray', (d) => {
        const hasTests = d.data.data ? d.data.data.has_tests : false;
        return hasTests ? '' : '3,2';
      })
      .style('cursor', 'pointer')
      .on('mouseover', (event, d) => {
        const nd = d.data.data || {};
        const name = nd.qualified_name || nd.symbol_id || '?';
        const intent = nd.sir_intent || '';
        const drift = (nd.drift_score || 0).toFixed(2);
        const pr = (nd.pagerank || 0).toFixed(3);
        const file = nd.file_path || '';
        tip().show(event,
          `<strong>${name}</strong>` +
          (intent ? `<br/><em>${intent.substring(0, 120)}</em>` : '') +
          `<br/>drift ${drift} / importance ${pr}` +
          `<br/><span style="opacity:0.7">${file}</span>`
        );
      })
      .on('mouseout', () => tip().hide())
      .on('click', (event, d) => {
        const nd = d.data.data || {};
        if (event.shiftKey) {
          const encoded = encodeURIComponent(nd.symbol_id || '');
          htmx.ajax('GET', `/dashboard/frag/symbol/${encoded}`, {
            target: '#main-content',
            pushURL: `/dashboard/symbol/${encoded}`,
          });
        } else {
          const symInput = document.getElementById('blast-symbol-id');
          const textInput = document.getElementById('blast-symbol-input');
          if (symInput && nd.symbol_id) {
            symInput.value = nd.symbol_id;
            if (textInput) textInput.value = nd.qualified_name || nd.symbol_id;
            load();
          }
        }
      });

    if (window.AetherAnimate) window.AetherAnimate.enterTransition(circles);

    // Labels for nodes with enough importance
    nodes.filter((d) => (d.data.data ? d.data.data.pagerank || 0 : 0) > 0.01 || d.depth === 0)
      .append('text')
      .attr('dy', '0.31em')
      .attr('x', (d) => d.x < Math.PI === !d.children ? 6 : -6)
      .attr('text-anchor', (d) => d.x < Math.PI === !d.children ? 'start' : 'end')
      .attr('transform', (d) => {
        if (d.depth === 0) return '';
        return `rotate(${d.x * 180 / Math.PI - 90})${d.x >= Math.PI ? ' rotate(180)' : ''}`;
      })
      .attr('fill', () => window.AetherPalette ? window.AetherPalette.text() : '#64748b')
      .attr('font-size', 10)
      .text((d) => {
        const name = d.data.data ? (d.data.data.qualified_name || '') : '';
        return name.length > 25 ? name.slice(-25) : name;
      });
  }

  function load() {
    const container = document.getElementById('blast-radius-chart');
    if (!container) return;
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

    const depthSlider = document.getElementById('blast-depth');
    const couplingSlider = document.getElementById('blast-min-coupling');
    if (depthSlider) depthSlider.addEventListener('change', load);
    if (couplingSlider) couplingSlider.addEventListener('change', load);

    load();
  };
})();
