(function () {
  function fetchJson(url) {
    return fetch(url, { headers: { Accept: 'application/json' } }).then((r) => r.json());
  }

  function empty(container, message) {
    container.innerHTML = `<div class="chart-empty"><div class="empty-state"><div class="empty-state-title">${message}</div></div></div>`;
  }

  function layerColor(name) {
    const lower = String(name || '').toLowerCase();
    if (lower.includes('interface')) return '#0ea5a8';
    if (lower.includes('core')) return '#ea7d28';
    if (lower.includes('data')) return '#5e6db3';
    if (lower.includes('wire')) return '#b7871d';
    if (lower.includes('connect')) return '#1b9a59';
    if (lower.includes('test')) return '#c94a34';
    if (lower.includes('util')) return '#64748b';
    return '#0ea5a8';
  }

  window.initAnatomyGraph = function initAnatomyGraph() {
    const container = document.getElementById('anatomy-layer-graph');
    if (!container) return;

    fetchJson('/api/v1/anatomy').then((json) => {
      const graph = json?.data?.simplified_graph;
      const nodesRaw = graph?.nodes || [];
      const edgesRaw = graph?.edges || [];
      if (!Array.isArray(nodesRaw) || nodesRaw.length === 0) {
        empty(container, 'No layer graph data');
        return;
      }

      container.innerHTML = '';
      const width = container.clientWidth || 900;
      const height = Math.max(420, container.clientHeight || 420);
      const svg = d3.select(container).append('svg').attr('width', width).attr('height', height);

      const root = svg.append('g');
      svg.call(d3.zoom().scaleExtent([0.6, 3]).on('zoom', (event) => root.attr('transform', event.transform)));

      const nodes = nodesRaw.map((n) => ({
        id: n.id,
        symbol_count: Number(n.symbol_count || 0),
      }));
      const nodeById = new Map(nodes.map((n) => [n.id, n]));
      const edges = edgesRaw
        .filter((e) => nodeById.has(e.source) && nodeById.has(e.target))
        .map((e) => ({
          source: e.source,
          target: e.target,
          weight: Number(e.weight || 0),
        }));

      const maxCount = d3.max(nodes, (n) => n.symbol_count) || 1;
      const radius = d3.scaleSqrt().domain([1, maxCount]).range([18, 42]);
      const stroke = d3.scaleLinear().domain([1, d3.max(edges, (e) => e.weight) || 1]).range([1, 4]);

      const link = root
        .selectAll('line')
        .data(edges)
        .join('line')
        .attr('stroke', '#94a3b8')
        .attr('stroke-opacity', 0.45)
        .attr('stroke-width', (d) => stroke(d.weight));

      const node = root
        .selectAll('circle')
        .data(nodes)
        .join('circle')
        .attr('r', (d) => radius(Math.max(1, d.symbol_count)))
        .attr('fill', (d) => layerColor(d.id))
        .attr('fill-opacity', 0.88)
        .attr('stroke', '#e2e8f0')
        .attr('stroke-width', 1.2)
        .call(
          d3
            .drag()
            .on('start', (event, d) => {
              if (!event.active) simulation.alphaTarget(0.25).restart();
              d.fx = d.x;
              d.fy = d.y;
            })
            .on('drag', (event, d) => {
              d.fx = event.x;
              d.fy = event.y;
            })
            .on('end', (event, d) => {
              if (!event.active) simulation.alphaTarget(0);
              d.fx = null;
              d.fy = null;
            })
        );

      const labels = root
        .selectAll('text.layer-label')
        .data(nodes)
        .join('text')
        .attr('class', 'layer-label')
        .attr('text-anchor', 'middle')
        .attr('font-size', 11)
        .attr('fill', '#0f172a')
        .style('pointer-events', 'none')
        .text((d) => `${d.id} (${d.symbol_count})`);

      const edgeLabels = root
        .selectAll('text.edge-label')
        .data(edges)
        .join('text')
        .attr('class', 'edge-label')
        .attr('text-anchor', 'middle')
        .attr('font-size', 10)
        .attr('fill', '#64748b')
        .style('pointer-events', 'none')
        .text((d) => (d.weight > 0 ? String(d.weight) : ''));

      const simulation = d3
        .forceSimulation(nodes)
        .force('link', d3.forceLink(edges).id((d) => d.id).distance((d) => 90 - Math.min(40, d.weight * 3)))
        .force('charge', d3.forceManyBody().strength(-420))
        .force('center', d3.forceCenter(width / 2, height / 2))
        .force('collision', d3.forceCollide().radius((d) => radius(Math.max(1, d.symbol_count)) + 8));

      simulation.on('tick', () => {
        link
          .attr('x1', (d) => d.source.x)
          .attr('y1', (d) => d.source.y)
          .attr('x2', (d) => d.target.x)
          .attr('y2', (d) => d.target.y);

        node.attr('cx', (d) => d.x).attr('cy', (d) => d.y);

        labels.attr('x', (d) => d.x).attr('y', (d) => d.y + 3);

        edgeLabels
          .attr('x', (d) => (d.source.x + d.target.x) / 2)
          .attr('y', (d) => (d.source.y + d.target.y) / 2);
      });
    }).catch(() => {
      empty(container, 'Failed to load layer graph');
    });
  };
})();
