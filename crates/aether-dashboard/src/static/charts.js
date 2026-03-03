(function () {
  const BASE_COLORS = {
    text: '#64748b',
    border: '#cbd5e1',
    node: '#0ea5a8',
    warn: '#f59e0b',
    danger: '#ef4444',
    ok: '#10b981',
  };

  function getTooltip() {
    return window.AetherTooltip || { show: () => {}, hide: () => {} };
  }

  function riskColor(value) {
    if (window.AetherTheme && window.AetherTheme.riskColor) {
      return window.AetherTheme.riskColor(value);
    }
    return d3.interpolateRgb(BASE_COLORS.ok, BASE_COLORS.danger)(Math.max(0, Math.min(1, value || 0)));
  }

  function fetchJson(url) {
    return fetch(url, { headers: { 'Accept': 'application/json' } }).then((r) => r.json());
  }

  function empty(container, message) {
    container.innerHTML = `<div class="chart-empty"><div class="empty-state"><div class="empty-state-title">${message}</div></div></div>`;
  }

  window.initOverviewCharts = function initOverviewCharts() {
    const container = document.getElementById('overview-chart');
    if (!container) return;

    fetchJson('/api/v1/overview').then((json) => {
      const langs = Object.entries(json?.data?.languages || {}).sort((a, b) => b[1] - a[1]);
      if (!langs.length) {
        empty(container, 'No overview data');
        return;
      }

      container.innerHTML = '';
      const width = container.clientWidth || 480;
      const margin = { top: 8, right: 20, bottom: 8, left: 90 };
      const barH = 28;
      const height = langs.length * barH + margin.top + margin.bottom;
      const svg = d3.select(container).append('svg').attr('width', width).attr('height', height);
      const g = svg.append('g').attr('transform', `translate(${margin.left},${margin.top})`);
      const x = d3.scaleLinear().domain([0, d3.max(langs, (d) => d[1])]).range([0, width - margin.left - margin.right]);
      const y = d3.scaleBand().domain(langs.map((d) => d[0])).range([0, langs.length * barH]).padding(0.3);

      g.selectAll('rect')
        .data(langs)
        .join('rect')
        .attr('x', 0)
        .attr('y', (d) => y(d[0]))
        .attr('height', y.bandwidth())
        .attr('width', (d) => x(d[1]))
        .attr('rx', 3)
        .attr('fill', '#0ea5a8');

      g.selectAll('text.label')
        .data(langs)
        .join('text')
        .attr('x', -8)
        .attr('y', (d) => y(d[0]) + y.bandwidth() / 2)
        .attr('dy', '0.35em')
        .attr('text-anchor', 'end')
        .attr('fill', BASE_COLORS.text)
        .text((d) => d[0]);

      g.selectAll('text.value')
        .data(langs)
        .join('text')
        .attr('x', (d) => x(d[1]) + 6)
        .attr('y', (d) => y(d[0]) + y.bandwidth() / 2)
        .attr('dy', '0.35em')
        .attr('fill', BASE_COLORS.text)
        .text((d) => d[1]);
    }).catch(() => empty(container, 'Failed to load overview'));
  };

  window.initGraph = function initGraph() {
    const container = document.getElementById('graph-container');
    if (!container) return;

    const params = new URLSearchParams();
    const root = document.getElementById('graph-root');
    if (root && root.value.trim()) params.set('root', root.value.trim());
    params.set('depth', '2');
    params.set('limit', '220');

    fetchJson(`/api/v1/graph?${params.toString()}`).then((json) => {
      const data = json?.data;
      if (!data || !Array.isArray(data.nodes) || data.nodes.length === 0) {
        empty(container, 'No graph data');
        return;
      }

      container.innerHTML = '';
      const width = container.clientWidth || 900;
      const height = Math.max(520, container.clientHeight || 520);
      const svg = d3.select(container).append('svg').attr('width', width).attr('height', height);
      const rootG = svg.append('g');
      const tip = getTooltip();

      const nodes = data.nodes.map((n) => ({ ...n }));
      const nodeById = new Map(nodes.map((n) => [n.id, n]));
      const edges = (data.edges || []).filter((e) => nodeById.has(e.source) && nodeById.has(e.target)).map((e) => ({ ...e }));

      svg.call(d3.zoom().scaleExtent([0.2, 4]).on('zoom', (event) => rootG.attr('transform', event.transform)));

      const prScale = d3.scaleSqrt()
        .domain([0, d3.max(nodes, (n) => n.pagerank || 0) || 1])
        .range([4, 14]);

      const sim = d3.forceSimulation(nodes)
        .force('link', d3.forceLink(edges).id((d) => d.id).distance(70))
        .force('charge', d3.forceManyBody().strength(-180))
        .force('center', d3.forceCenter(width / 2, height / 2))
        .force('collision', d3.forceCollide().radius((d) => prScale(d.pagerank || 0) + 3));

      const link = rootG.selectAll('line.link')
        .data(edges)
        .join('line')
        .attr('stroke', '#94a3b8')
        .attr('stroke-opacity', 0.3)
        .attr('stroke-width', 1.2);

      const node = rootG.selectAll('circle.node')
        .data(nodes)
        .join('circle')
        .attr('r', (d) => prScale(d.pagerank || 0))
        .attr('fill', (d) => riskColor(d.risk_score || 0))
        .attr('stroke', (d) => d.sir_exists ? '#0ea5a8' : '#94a3b8')
        .attr('stroke-width', 1.5)
        .style('cursor', 'pointer')
        .on('mouseover', function (event, d) {
          tip.show(event, `<strong>${d.label}</strong><br/>${d.file}<br/>risk ${(d.risk_score || 0).toFixed(2)} / importance ${(d.pagerank || 0).toFixed(2)}`);
        })
        .on('mouseout', () => tip.hide())
        .on('click', function (_event, d) {
          const encoded = encodeURIComponent(d.id);
          htmx.ajax('GET', `/dashboard/frag/symbol/${encoded}`, {
            target: '#main-content',
            pushURL: `/dashboard/symbol/${encoded}`,
          });
        })
        .on('contextmenu', function (event, d) {
          event.preventDefault();
          const choice = prompt('Action: 1=Blast Radius, 2=Trace Causes');
          if (choice === '1') {
            htmx.ajax('GET', `/dashboard/frag/blast-radius?symbol_id=${d.id}`, '#main-content');
          } else if (choice === '2') {
            htmx.ajax('GET', `/dashboard/frag/causal`, '#main-content');
            setTimeout(() => {
              const hidden = document.getElementById('causal-symbol-id');
              if (hidden) hidden.value = d.id;
              if (window.initCausalExplorer) window.initCausalExplorer();
            }, 40);
          }
        })
        .call(d3.drag()
          .on('start', (event, d) => { if (!event.active) sim.alphaTarget(0.3).restart(); d.fx = d.x; d.fy = d.y; })
          .on('drag', (event, d) => { d.fx = event.x; d.fy = event.y; })
          .on('end', (event, d) => { if (!event.active) sim.alphaTarget(0); d.fx = null; d.fy = null; }));

      const labels = rootG.selectAll('text.label')
        .data(nodes.filter((n) => (n.pagerank || 0) > 0))
        .join('text')
        .attr('font-size', 10)
        .attr('fill', '#64748b')
        .attr('text-anchor', 'middle')
        .text((d) => d.label);

      sim.on('tick', () => {
        link
          .attr('x1', (d) => d.source.x)
          .attr('y1', (d) => d.source.y)
          .attr('x2', (d) => d.target.x)
          .attr('y2', (d) => d.target.y);

        node
          .attr('cx', (d) => d.x)
          .attr('cy', (d) => d.y);

        labels
          .attr('x', (d) => d.x)
          .attr('y', (d) => d.y - 10);
      });
    }).catch(() => empty(container, 'Failed to load graph'));
  };

  window.initDriftChart = function initDriftChart() {
    const container = document.getElementById('drift-chart');
    if (!container) return;

    fetchJson('/api/v1/drift').then((json) => {
      const rows = json?.data?.drift_entries || [];
      if (!rows.length) {
        empty(container, 'No drift data');
        return;
      }

      container.innerHTML = '';
      const width = container.clientWidth || 760;
      const height = 320;
      const margin = { top: 20, right: 20, bottom: 40, left: 50 };
      const svg = d3.select(container).append('svg').attr('width', width).attr('height', height);
      const g = svg.append('g').attr('transform', `translate(${margin.left},${margin.top})`);

      const points = rows.map((r) => ({
        x: new Date(Number(r.detected_at || 0)),
        y: Number(r.drift_magnitude || r.drift_score || 0),
        symbol_id: r.symbol_id,
        name: r.symbol_name,
      }));

      const x = d3.scaleTime().domain(d3.extent(points, (d) => d.x)).range([0, width - margin.left - margin.right]);
      const y = d3.scaleLinear().domain([0, d3.max(points, (d) => d.y) || 1]).nice().range([height - margin.top - margin.bottom, 0]);

      g.append('g').attr('transform', `translate(0,${height - margin.top - margin.bottom})`).call(d3.axisBottom(x).ticks(5));
      g.append('g').call(d3.axisLeft(y).ticks(5));

      g.selectAll('circle')
        .data(points)
        .join('circle')
        .attr('cx', (d) => x(d.x))
        .attr('cy', (d) => y(d.y))
        .attr('r', 4)
        .attr('fill', (d) => d.y >= 0.6 ? BASE_COLORS.danger : d.y >= 0.3 ? BASE_COLORS.warn : BASE_COLORS.ok)
        .style('cursor', 'pointer')
        .on('click', (_e, d) => {
          const encoded = encodeURIComponent(d.symbol_id);
          htmx.ajax('GET', `/dashboard/frag/symbol/${encoded}`, {
            target: '#main-content',
            pushURL: `/dashboard/symbol/${encoded}`,
          });
        });
    }).catch(() => empty(container, 'Failed to load drift'));
  };

  window.initHeatmap = function initHeatmap() {
    const container = document.getElementById('heatmap-container');
    if (!container) return;

    fetchJson('/api/v1/coupling?limit=80').then((json) => {
      const pairs = json?.data?.pairs || [];
      if (!pairs.length) {
        empty(container, 'No coupling data');
        return;
      }

      container.innerHTML = '';
      const files = new Set();
      pairs.forEach((p) => { files.add(p.file_a); files.add(p.file_b); });
      const labels = Array.from(files).slice(0, 30);
      const size = Math.max(12, Math.floor((container.clientWidth - 140) / labels.length));
      const margin = { top: 90, right: 20, bottom: 20, left: 120 };
      const dim = labels.length * size;
      const width = dim + margin.left + margin.right;
      const height = dim + margin.top + margin.bottom;
      const svg = d3.select(container).append('svg').attr('width', width).attr('height', height);
      const g = svg.append('g').attr('transform', `translate(${margin.left},${margin.top})`);

      const x = d3.scaleBand().domain(labels).range([0, dim]).padding(0.06);
      const y = d3.scaleBand().domain(labels).range([0, dim]).padding(0.06);

      const m = new Map();
      pairs.forEach((p) => {
        const k1 = `${p.file_a}|${p.file_b}`;
        const k2 = `${p.file_b}|${p.file_a}`;
        m.set(k1, p);
        m.set(k2, p);
      });

      labels.forEach((a) => {
        labels.forEach((b) => {
          const pair = m.get(`${a}|${b}`);
          const score = pair ? Number(pair.coupling_score || 0) : 0;
          if (!pair && a !== b) return;

          const signal = (pair?.coupling_type || '').toLowerCase();
          let hue = '#94a3b8';
          if (signal.includes('struct')) hue = '#3b82f6';
          if (signal.includes('semantic')) hue = '#22c55e';
          if (signal.includes('temporal')) hue = '#f59e0b';

          g.append('rect')
            .attr('x', x(b))
            .attr('y', y(a))
            .attr('width', x.bandwidth())
            .attr('height', y.bandwidth())
            .attr('fill', a === b ? '#cbd5e1' : d3.color(hue).copy({ opacity: Math.max(0.2, Math.min(1, score)) }))
            .attr('stroke', '#e2e8f0')
            .attr('stroke-width', 0.5);
        });
      });
    }).catch(() => empty(container, 'Failed to load coupling'));
  };

  window.initHealthChart = function initHealthChart() {
    const container = document.getElementById('health-chart');
    if (!container) return;

    fetchJson('/api/v1/health').then((json) => {
      const d = json?.data;
      if (!d || !d.dimensions) {
        empty(container, 'No health data');
        return;
      }

      const entries = [
        ['Understanding Coverage', Number(d.dimensions.sir_coverage || 0)],
        ['Test Coverage', Number(d.dimensions.test_coverage || 0)],
        ['Graph Connectivity', Number(d.dimensions.graph_connectivity || 0)],
        ['Coupling Health', Number(d.dimensions.coupling_health || 0)],
        ['Change Risk Health', Number(d.dimensions.drift_health || 0)],
      ];

      container.innerHTML = '';
      const width = container.clientWidth || 580;
      const margin = { top: 16, right: 40, bottom: 8, left: 130 };
      const barH = 34;
      const height = entries.length * barH + margin.top + margin.bottom;
      const svg = d3.select(container).append('svg').attr('width', width).attr('height', height);
      const g = svg.append('g').attr('transform', `translate(${margin.left},${margin.top})`);
      const x = d3.scaleLinear().domain([0, 1]).range([0, width - margin.left - margin.right]);
      const y = d3.scaleBand().domain(entries.map((d) => d[0])).range([0, entries.length * barH]).padding(0.35);

      g.selectAll('rect.track')
        .data(entries)
        .join('rect')
        .attr('x', 0)
        .attr('y', (d) => y(d[0]))
        .attr('width', x(1))
        .attr('height', y.bandwidth())
        .attr('fill', '#e2e8f0');

      g.selectAll('rect.bar')
        .data(entries)
        .join('rect')
        .attr('x', 0)
        .attr('y', (d) => y(d[0]))
        .attr('width', (d) => x(d[1]))
        .attr('height', y.bandwidth())
        .attr('fill', (d) => riskColor(1 - d[1]));

      g.selectAll('text.label')
        .data(entries)
        .join('text')
        .attr('x', -8)
        .attr('y', (d) => y(d[0]) + y.bandwidth() / 2)
        .attr('dy', '0.35em')
        .attr('text-anchor', 'end')
        .attr('fill', BASE_COLORS.text)
        .text((d) => d[0]);

      g.selectAll('text.value')
        .data(entries)
        .join('text')
        .attr('x', (d) => x(d[1]) + 8)
        .attr('y', (d) => y(d[0]) + y.bandwidth() / 2)
        .attr('dy', '0.35em')
        .attr('fill', BASE_COLORS.text)
        .text((d) => `${Math.round(d[1] * 100)}%`);
    }).catch(() => empty(container, 'Failed to load health'));
  };
})();
