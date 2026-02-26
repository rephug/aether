/* ── AETHER Dashboard — D3 Chart Definitions ────────────
   Each function:
   1. Checks if its container div exists (HTMX may not have loaded the page)
   2. Fetches JSON from /api/v1/*
   3. Renders the chart using D3
   4. Handles empty data gracefully
   ──────────────────────────────────────────────────────── */

const COLORS = {
  cyan:   '#39bae6',
  orange: '#ff8f40',
  green:  '#7fd962',
  red:    '#f26d78',
  purple: '#d2a6ff',
  yellow: '#e6b450',
  bg:     '#10151c',
  border: '#1e2633',
  text:   '#8a919b',
  muted:  '#5c6370',
};

const LANG_COLORS = {
  rust:       '#ff8f40',
  typescript: '#39bae6',
  python:     '#7fd962',
  javascript: '#e6b450',
  go:         '#39bae6',
  java:       '#f26d78',
  c:          '#d2a6ff',
  cpp:        '#d2a6ff',
  csharp:     '#d2a6ff',
  ruby:       '#f26d78',
  default:    '#5c6370',
};

function langColor(lang) {
  return LANG_COLORS[lang?.toLowerCase()] || LANG_COLORS.default;
}

/* ── Shared: Tooltip helper ───────────────────────────── */
function createTooltip(container) {
  const existing = container.querySelector('.d3-tooltip');
  if (existing) return existing;
  const tip = document.createElement('div');
  tip.className = 'd3-tooltip';
  container.appendChild(tip);
  return tip;
}
function showTooltip(tip, html, x, y) {
  tip.innerHTML = html;
  tip.style.left = (x + 12) + 'px';
  tip.style.top = (y - 8) + 'px';
  tip.classList.add('visible');
}
function hideTooltip(tip) {
  tip.classList.remove('visible');
}

/* ── Shared: Empty state ──────────────────────────────── */
function showEmpty(container, message, command) {
  container.innerHTML = `
    <div class="chart-empty">
      <div class="empty-state">
        <svg class="empty-state-icon" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1.5">
          <path stroke-linecap="round" stroke-linejoin="round" d="M20.25 6.375c0 2.278-3.694 4.125-8.25 4.125S3.75 8.653 3.75 6.375m16.5 0c0-2.278-3.694-4.125-8.25-4.125S3.75 4.097 3.75 6.375m16.5 0v11.25c0 2.278-3.694 4.125-8.25 4.125s-8.25-1.847-8.25-4.125V6.375m16.5 0v3.75m-16.5-3.75v3.75m16.5 0v3.75C20.25 16.153 16.556 18 12 18s-8.25-1.847-8.25-4.125v-3.75m16.5 0c0 2.278-3.694 4.125-8.25 4.125s-8.25-1.847-8.25-4.125" />
        </svg>
        <div class="empty-state-title">${message}</div>
        ${command ? `<code class="empty-state-cmd">${command}</code>` : ''}
      </div>
    </div>`;
}

/* ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
   OVERVIEW PAGE — Language breakdown bar chart + SIR gauge
   ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ */

function initOverviewCharts() {
  const container = document.getElementById('overview-chart');
  if (!container) return;

  fetch('/api/v1/overview')
    .then(r => r.json())
    .then(json => {
      const d = json.data;
      if (json.meta) checkStaleness(json.meta);

      // Update footer
      const fb = document.getElementById('footer-backend');
      if (fb && d.graph_backend) fb.textContent = d.graph_backend;

      // Language breakdown → horizontal bar chart
      const langs = d.languages || {};
      const entries = Object.entries(langs).sort((a, b) => b[1] - a[1]);
      if (entries.length === 0) {
        showEmpty(container, 'No indexed files yet', 'aether index');
        return;
      }

      const margin = { top: 8, right: 48, bottom: 8, left: 90 };
      const barH = 28;
      const height = entries.length * barH + margin.top + margin.bottom;
      const width = container.clientWidth || 400;

      container.innerHTML = '';
      const svg = d3.select(container)
        .append('svg')
        .attr('width', width)
        .attr('height', height);

      const g = svg.append('g').attr('transform', `translate(${margin.left},${margin.top})`);
      const innerW = width - margin.left - margin.right;

      const x = d3.scaleLinear()
        .domain([0, d3.max(entries, e => e[1]) || 1])
        .range([0, innerW]);

      const y = d3.scaleBand()
        .domain(entries.map(e => e[0]))
        .range([0, entries.length * barH])
        .padding(0.35);

      // Bars
      g.selectAll('rect.bar')
        .data(entries)
        .join('rect')
        .attr('class', 'bar')
        .attr('x', 0)
        .attr('y', e => y(e[0]))
        .attr('width', 0)
        .attr('height', y.bandwidth())
        .attr('rx', 3)
        .attr('fill', e => langColor(e[0]))
        .attr('opacity', 0.85)
        .transition()
        .duration(600)
        .ease(d3.easeCubicOut)
        .attr('width', e => x(e[1]));

      // Labels (language name)
      g.selectAll('text.label')
        .data(entries)
        .join('text')
        .attr('class', 'label')
        .attr('x', -8)
        .attr('y', e => y(e[0]) + y.bandwidth() / 2)
        .attr('dy', '0.35em')
        .attr('text-anchor', 'end')
        .attr('fill', COLORS.text)
        .attr('font-family', '"JetBrains Mono", monospace')
        .attr('font-size', '11px')
        .text(e => e[0]);

      // Count labels
      g.selectAll('text.count')
        .data(entries)
        .join('text')
        .attr('class', 'count')
        .attr('x', e => x(e[1]) + 6)
        .attr('y', e => y(e[0]) + y.bandwidth() / 2)
        .attr('dy', '0.35em')
        .attr('fill', COLORS.muted)
        .attr('font-family', '"JetBrains Mono", monospace')
        .attr('font-size', '10px')
        .text(e => e[1].toLocaleString());
    })
    .catch(err => {
      console.error('Overview chart error:', err);
      showEmpty(container, 'Failed to load overview data');
    });
}

/* ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
   GRAPH PAGE — Force-directed dependency graph
   ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ */

function initGraph() {
  const container = document.getElementById('graph-container');
  if (!container) return;

  const params = new URLSearchParams();
  const rootInput = document.getElementById('graph-root');
  if (rootInput && rootInput.value) params.set('root', rootInput.value);
  params.set('limit', '200');
  params.set('depth', '2');

  fetch(`/api/v1/graph?${params}`)
    .then(r => r.json())
    .then(json => {
      const d = json.data;
      if (!d.nodes || d.nodes.length === 0) {
        showEmpty(container, 'No graph data available', 'aether index');
        return;
      }

      container.innerHTML = '';
      const tip = createTooltip(container);
      const width = container.clientWidth || 800;
      const height = Math.max(500, container.clientHeight || 500);

      const svg = d3.select(container)
        .append('svg')
        .attr('width', width)
        .attr('height', height);

      // Zoom layer
      const g = svg.append('g');
      svg.call(d3.zoom()
        .scaleExtent([0.1, 4])
        .on('zoom', (event) => g.attr('transform', event.transform)));

      // Build node index for edge lookup
      const nodeMap = new Map(d.nodes.map(n => [n.id, n]));

      // Filter edges to only those with valid source/target
      const links = (d.edges || [])
        .filter(e => nodeMap.has(e.source) && nodeMap.has(e.target))
        .map(e => ({ ...e }));

      // Color by file path (hash to hue)
      function fileHue(file) {
        let hash = 0;
        for (let i = 0; i < (file || '').length; i++) {
          hash = ((hash << 5) - hash + file.charCodeAt(i)) | 0;
        }
        return `hsl(${Math.abs(hash) % 360}, 55%, 60%)`;
      }

      function nodeRadius(kind) {
        if (kind === 'module' || kind === 'struct' || kind === 'class') return 7;
        if (kind === 'trait' || kind === 'interface') return 6;
        return 5;
      }

      const sim = d3.forceSimulation(d.nodes)
        .force('link', d3.forceLink(links).id(n => n.id).distance(60))
        .force('charge', d3.forceManyBody().strength(-120))
        .force('center', d3.forceCenter(width / 2, height / 2))
        .force('collision', d3.forceCollide().radius(12));

      const link = g.selectAll('line.graph-edge')
        .data(links)
        .join('line')
        .attr('class', 'graph-edge');

      const node = g.selectAll('circle.graph-node')
        .data(d.nodes)
        .join('circle')
        .attr('class', 'graph-node')
        .attr('r', n => nodeRadius(n.kind))
        .attr('fill', n => fileHue(n.file))
        .attr('stroke', n => n.sir_exists ? 'rgba(57,186,230,0.4)' : 'transparent')
        .attr('stroke-width', 1.5)
        .on('mouseover', (event, n) => {
          const rect = container.getBoundingClientRect();
          showTooltip(tip,
            `<strong style="color:#e6e1cf">${n.label || n.id}</strong><br/>
             <span style="color:${COLORS.muted}">${n.kind} · ${n.file || '—'}</span>`,
            event.clientX - rect.left, event.clientY - rect.top);
        })
        .on('mouseout', () => hideTooltip(tip))
        .on('click', (event, n) => {
          htmx.ajax('GET', `/dashboard/frag/symbol/${n.id}`, '#detail-panel');
        })
        .call(d3.drag()
          .on('start', (event, n) => { if (!event.active) sim.alphaTarget(0.3).restart(); n.fx = n.x; n.fy = n.y; })
          .on('drag', (event, n) => { n.fx = event.x; n.fy = event.y; })
          .on('end', (event, n) => { if (!event.active) sim.alphaTarget(0); n.fx = null; n.fy = null; }));

      // Labels for larger nodes
      const label = g.selectAll('text.graph-label')
        .data(d.nodes.filter(n => nodeRadius(n.kind) >= 6))
        .join('text')
        .attr('class', 'graph-label')
        .attr('dy', -10)
        .attr('text-anchor', 'middle')
        .text(n => n.label || n.id);

      sim.on('tick', () => {
        link
          .attr('x1', l => l.source.x)
          .attr('y1', l => l.source.y)
          .attr('x2', l => l.target.x)
          .attr('y2', l => l.target.y);
        node
          .attr('cx', n => n.x)
          .attr('cy', n => n.y);
        label
          .attr('x', n => n.x)
          .attr('y', n => n.y);
      });

      // Truncation notice
      if (d.truncated) {
        svg.append('text')
          .attr('x', width - 10).attr('y', 20)
          .attr('text-anchor', 'end')
          .attr('fill', COLORS.yellow)
          .attr('font-size', '11px')
          .attr('font-family', '"JetBrains Mono", monospace')
          .text(`Showing ${d.nodes.length} of ${d.total_nodes} nodes`);
      }
    })
    .catch(err => {
      console.error('Graph chart error:', err);
      showEmpty(container, 'Failed to load graph data');
    });
}

/* ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
   DRIFT PAGE — Scatter plot of drift magnitude over time
   ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ */

function initDriftChart() {
  const container = document.getElementById('drift-chart');
  if (!container) return;

  fetch('/api/v1/drift')
    .then(r => r.json())
    .then(json => {
      const d = json.data;
      if (!d.drift_entries || d.drift_entries.length === 0) {
        showEmpty(container, 'No drift data yet', 'aether drift-report');
        return;
      }

      container.innerHTML = '';
      const tip = createTooltip(container);
      const margin = { top: 20, right: 20, bottom: 40, left: 50 };
      const width = container.clientWidth || 700;
      const height = 320;

      const svg = d3.select(container)
        .append('svg')
        .attr('width', width)
        .attr('height', height);

      const g = svg.append('g').attr('transform', `translate(${margin.left},${margin.top})`);
      const innerW = width - margin.left - margin.right;
      const innerH = height - margin.top - margin.bottom;

      const entries = d.drift_entries.map(e => ({
        ...e,
        time: new Date(e.detected_at),
        mag: +e.drift_magnitude || +e.drift_score || 0,
      }));

      const x = d3.scaleTime()
        .domain(d3.extent(entries, e => e.time))
        .range([0, innerW])
        .nice();

      const y = d3.scaleLinear()
        .domain([0, d3.max(entries, e => e.mag) * 1.1 || 1])
        .range([innerH, 0]);

      // Grid lines
      g.append('g').attr('transform', `translate(0,${innerH})`)
        .call(d3.axisBottom(x).ticks(6).tickSize(-innerH).tickFormat(''))
        .selectAll('line').attr('stroke', COLORS.border).attr('stroke-opacity', 0.5);
      g.append('g')
        .call(d3.axisLeft(y).ticks(5).tickSize(-innerW).tickFormat(''))
        .selectAll('line').attr('stroke', COLORS.border).attr('stroke-opacity', 0.5);

      // Axes
      g.append('g').attr('transform', `translate(0,${innerH})`).call(d3.axisBottom(x).ticks(6));
      g.append('g').call(d3.axisLeft(y).ticks(5));

      // Axis label
      g.append('text')
        .attr('x', innerW / 2).attr('y', innerH + 34)
        .attr('text-anchor', 'middle')
        .attr('fill', COLORS.muted).attr('font-size', '10px')
        .text('Detection Time');
      g.append('text')
        .attr('transform', 'rotate(-90)')
        .attr('x', -innerH / 2).attr('y', -38)
        .attr('text-anchor', 'middle')
        .attr('fill', COLORS.muted).attr('font-size', '10px')
        .text('Drift Magnitude');

      const driftColor = {
        semantic:    COLORS.orange,
        structural:  COLORS.purple,
        boundary:    COLORS.red,
        emerging_hub: COLORS.yellow,
      };

      // Dots
      g.selectAll('circle.dot')
        .data(entries)
        .join('circle')
        .attr('class', 'dot')
        .attr('cx', e => x(e.time))
        .attr('cy', e => y(e.mag))
        .attr('r', 0)
        .attr('fill', e => driftColor[e.drift_type] || COLORS.cyan)
        .attr('opacity', 0.8)
        .style('cursor', 'pointer')
        .on('mouseover', (event, e) => {
          const rect = container.getBoundingClientRect();
          showTooltip(tip,
            `<strong style="color:#e6e1cf">${e.symbol_name || e.symbol_id}</strong><br/>
             <span style="color:${driftColor[e.drift_type] || COLORS.cyan}">${e.drift_type || 'drift'}</span>
             · magnitude <strong>${e.mag.toFixed(2)}</strong>`,
            event.clientX - rect.left, event.clientY - rect.top);
        })
        .on('mouseout', () => hideTooltip(tip))
        .on('click', (event, e) => {
          htmx.ajax('GET', `/dashboard/frag/symbol/${e.symbol_id}`, '#detail-panel');
        })
        .transition().duration(400).delay((_, i) => i * 15)
        .attr('r', 5);
    })
    .catch(err => {
      console.error('Drift chart error:', err);
      showEmpty(container, 'Failed to load drift data');
    });
}

/* ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
   COUPLING PAGE — Heatmap or top-pairs bar chart
   ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ */

function initHeatmap() {
  const container = document.getElementById('heatmap-container');
  if (!container) return;

  fetch('/api/v1/coupling?limit=100')
    .then(r => r.json())
    .then(json => {
      const d = json.data;
      if (!d.pairs || d.pairs.length === 0) {
        showEmpty(container, 'No coupling data yet', 'aether mine-coupling');
        return;
      }

      container.innerHTML = '';
      const tip = createTooltip(container);

      // If too many unique files (>30), fall back to bar chart of top pairs
      const files = new Set();
      d.pairs.forEach(p => { files.add(p.file_a || p.symbol_a); files.add(p.file_b || p.symbol_b); });

      if (files.size > 30) {
        renderCouplingBars(container, tip, d.pairs);
        return;
      }

      // Full heatmap
      const labels = [...files].sort();
      const cellSize = Math.min(24, Math.floor((container.clientWidth - 120) / labels.length));
      const margin = { top: 80, right: 20, bottom: 20, left: 120 };
      const dim = labels.length * cellSize;
      const width = dim + margin.left + margin.right;
      const height = dim + margin.top + margin.bottom;

      const svg = d3.select(container)
        .append('svg')
        .attr('width', width)
        .attr('height', height);

      const g = svg.append('g').attr('transform', `translate(${margin.left},${margin.top})`);

      const x = d3.scaleBand().domain(labels).range([0, dim]).padding(0.05);
      const y = d3.scaleBand().domain(labels).range([0, dim]).padding(0.05);
      const color = d3.scaleSequential(d3.interpolateYlOrRd).domain([0, 1]);

      // Build lookup
      const pairMap = new Map();
      d.pairs.forEach(p => {
        const a = p.file_a || p.symbol_a;
        const b = p.file_b || p.symbol_b;
        pairMap.set(`${a}|${b}`, p);
        pairMap.set(`${b}|${a}`, p);
      });

      // Cells
      labels.forEach(a => {
        labels.forEach(b => {
          const pair = pairMap.get(`${a}|${b}`);
          const score = pair ? pair.coupling_score : 0;
          if (score === 0 && a !== b) return;

          g.append('rect')
            .attr('class', 'heatmap-cell')
            .attr('x', x(b))
            .attr('y', y(a))
            .attr('width', x.bandwidth())
            .attr('height', y.bandwidth())
            .attr('fill', a === b ? COLORS.border : color(score))
            .on('mouseover', (event) => {
              if (!pair) return;
              const rect = container.getBoundingClientRect();
              const s = pair.signals || {};
              showTooltip(tip,
                `<strong style="color:#e6e1cf">${a}</strong> ↔ <strong style="color:#e6e1cf">${b}</strong><br/>
                 Score: <strong>${score.toFixed(2)}</strong><br/>
                 <span style="color:${COLORS.muted}">temporal: ${(s.temporal||s.co_change||0).toFixed(2)} · structural: ${(s.structural||s.static_signal||0).toFixed(2)} · semantic: ${(s.semantic||0).toFixed(2)}</span>`,
                event.clientX - rect.left, event.clientY - rect.top);
            })
            .on('mouseout', () => hideTooltip(tip));
        });
      });

      // Axis labels
      g.selectAll('text.x-label')
        .data(labels)
        .join('text')
        .attr('class', 'heatmap-label')
        .attr('x', l => x(l) + x.bandwidth() / 2)
        .attr('y', -6)
        .attr('text-anchor', 'start')
        .attr('transform', l => `rotate(-45, ${x(l) + x.bandwidth()/2}, -6)`)
        .text(l => l.split('/').pop());

      g.selectAll('text.y-label')
        .data(labels)
        .join('text')
        .attr('class', 'heatmap-label')
        .attr('x', -6)
        .attr('y', l => y(l) + y.bandwidth() / 2)
        .attr('dy', '0.35em')
        .attr('text-anchor', 'end')
        .text(l => l.split('/').pop());
    })
    .catch(err => {
      console.error('Heatmap error:', err);
      showEmpty(container, 'Failed to load coupling data');
    });
}

/* Fallback: top coupling pairs as horizontal bar chart */
function renderCouplingBars(container, tip, pairs) {
  const top = pairs.slice(0, 25).sort((a, b) => b.coupling_score - a.coupling_score);
  const margin = { top: 8, right: 50, bottom: 8, left: 200 };
  const barH = 26;
  const height = top.length * barH + margin.top + margin.bottom;
  const width = container.clientWidth || 600;

  const svg = d3.select(container)
    .append('svg')
    .attr('width', width)
    .attr('height', height);

  const g = svg.append('g').attr('transform', `translate(${margin.left},${margin.top})`);
  const innerW = width - margin.left - margin.right;

  const x = d3.scaleLinear().domain([0, 1]).range([0, innerW]);
  const y = d3.scaleBand()
    .domain(top.map((p, i) => i))
    .range([0, top.length * barH])
    .padding(0.3);

  const color = d3.scaleSequential(d3.interpolateYlOrRd).domain([0, 1]);

  g.selectAll('rect')
    .data(top)
    .join('rect')
    .attr('x', 0)
    .attr('y', (_, i) => y(i))
    .attr('width', p => x(p.coupling_score))
    .attr('height', y.bandwidth())
    .attr('rx', 3)
    .attr('fill', p => color(p.coupling_score))
    .attr('opacity', 0.85);

  g.selectAll('text.pair-label')
    .data(top)
    .join('text')
    .attr('x', -6)
    .attr('y', (_, i) => y(i) + y.bandwidth() / 2)
    .attr('dy', '0.35em')
    .attr('text-anchor', 'end')
    .attr('fill', COLORS.text)
    .attr('font-family', '"JetBrains Mono", monospace')
    .attr('font-size', '9px')
    .text(p => `${(p.file_a||p.symbol_a).split('/').pop()} ↔ ${(p.file_b||p.symbol_b).split('/').pop()}`);

  g.selectAll('text.score-label')
    .data(top)
    .join('text')
    .attr('x', p => x(p.coupling_score) + 6)
    .attr('y', (_, i) => y(i) + y.bandwidth() / 2)
    .attr('dy', '0.35em')
    .attr('fill', COLORS.muted)
    .attr('font-family', '"JetBrains Mono", monospace')
    .attr('font-size', '10px')
    .text(p => p.coupling_score.toFixed(2));
}

/* ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
   HEALTH PAGE — Horizontal bar chart of health dimensions
   ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━ */

function initHealthChart() {
  const container = document.getElementById('health-chart');
  if (!container) return;

  fetch('/api/v1/health')
    .then(r => r.json())
    .then(json => {
      const d = json.data;
      if (!d.dimensions || d.analysis_available === false) {
        showEmpty(container, 'No health data available', 'aether health');
        return;
      }

      container.innerHTML = '';

      const dims = d.dimensions;
      const entries = [
        { label: 'SIR Coverage',    value: dims.sir_coverage || 0 },
        { label: 'Test Coverage',   value: dims.test_coverage || 0 },
        { label: 'Coupling Health', value: dims.coupling_health || 0 },
        { label: 'Drift Health',    value: dims.drift_health || 0 },
      ].filter(e => e.value != null);

      if (entries.length === 0) {
        showEmpty(container, 'No health dimensions available');
        return;
      }

      const margin = { top: 20, right: 60, bottom: 12, left: 120 };
      const barH = 36;
      const height = entries.length * barH + margin.top + margin.bottom;
      const width = container.clientWidth || 500;

      const svg = d3.select(container)
        .append('svg')
        .attr('width', width)
        .attr('height', height);

      const g = svg.append('g').attr('transform', `translate(${margin.left},${margin.top})`);
      const innerW = width - margin.left - margin.right;

      const x = d3.scaleLinear().domain([0, 1]).range([0, innerW]);
      const y = d3.scaleBand()
        .domain(entries.map(e => e.label))
        .range([0, entries.length * barH])
        .padding(0.4);

      function barColor(v) {
        if (v >= 0.8) return COLORS.green;
        if (v >= 0.5) return COLORS.yellow;
        return COLORS.red;
      }

      // Background tracks
      g.selectAll('rect.track')
        .data(entries)
        .join('rect')
        .attr('x', 0)
        .attr('y', e => y(e.label))
        .attr('width', innerW)
        .attr('height', y.bandwidth())
        .attr('rx', 4)
        .attr('fill', COLORS.border)
        .attr('opacity', 0.4);

      // Value bars
      g.selectAll('rect.bar')
        .data(entries)
        .join('rect')
        .attr('x', 0)
        .attr('y', e => y(e.label))
        .attr('width', 0)
        .attr('height', y.bandwidth())
        .attr('rx', 4)
        .attr('fill', e => barColor(e.value))
        .attr('opacity', 0.85)
        .transition()
        .duration(600)
        .ease(d3.easeCubicOut)
        .attr('width', e => x(e.value));

      // Labels
      g.selectAll('text.dim-label')
        .data(entries)
        .join('text')
        .attr('x', -8)
        .attr('y', e => y(e.label) + y.bandwidth() / 2)
        .attr('dy', '0.35em')
        .attr('text-anchor', 'end')
        .attr('fill', COLORS.text)
        .attr('font-size', '12px')
        .text(e => e.label);

      // Percentage labels
      g.selectAll('text.pct')
        .data(entries)
        .join('text')
        .attr('x', e => x(e.value) + 8)
        .attr('y', e => y(e.label) + y.bandwidth() / 2)
        .attr('dy', '0.35em')
        .attr('fill', e => barColor(e.value))
        .attr('font-family', '"JetBrains Mono", monospace')
        .attr('font-size', '12px')
        .attr('font-weight', '600')
        .text(e => `${Math.round(e.value * 100)}%`);

      // Overall score
      if (d.overall_score != null) {
        svg.append('text')
          .attr('x', width - 10).attr('y', 14)
          .attr('text-anchor', 'end')
          .attr('fill', barColor(d.overall_score))
          .attr('font-family', '"JetBrains Mono", monospace')
          .attr('font-size', '14px')
          .attr('font-weight', '700')
          .text(`Overall: ${Math.round(d.overall_score * 100)}%`);
      }
    })
    .catch(err => {
      console.error('Health chart error:', err);
      showEmpty(container, 'Failed to load health data');
    });
}
