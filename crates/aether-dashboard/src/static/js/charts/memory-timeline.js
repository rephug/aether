(function () {
  const tip = () => window.AetherTooltip || { show() {}, hide() {} };

  function empty(el, msg) {
    el.innerHTML = `<div class="chart-empty"><div class="empty-state"><div class="empty-state-title">${msg}</div></div></div>`;
  }

  const TYPE_COLORS = {
    structural: '#3b82f6',
    semantic: '#22c55e',
    memory: '#f59e0b',
    health: '#ef4444',
  };

  function draw(container, data, activeTypes) {
    container.innerHTML = '';
    const events = (data.events || []).filter((e) => activeTypes.has(e.event_type));
    if (!events.length) { empty(container, 'No events match the selected filters'); return; }

    const P = window.AetherPalette || {};
    const textColor = typeof P.text === 'function' ? P.text() : '#64748b';
    const gridColor = typeof P.gridLine === 'function' ? P.gridLine() : 'rgba(203,213,225,0.5)';

    const width = container.clientWidth || 800;
    const height = Math.max(420, container.clientHeight || 420);
    const margin = { top: 40, right: 20, bottom: 40, left: 60 };
    const plotW = width - margin.left - margin.right;
    const plotH = height - margin.top - margin.bottom;

    const svg = d3.select(container).append('svg').attr('width', width).attr('height', height);
    const g = svg.append('g').attr('transform', `translate(${margin.left},${margin.top})`);

    const dates = events.map((e) => new Date(e.timestamp));
    const x = d3.scaleTime().domain(d3.extent(dates)).range([0, plotW]);

    // Distribute events vertically by type to avoid overlap
    const typeOrder = ['structural', 'semantic', 'memory', 'health'];
    const yBand = d3.scaleBand().domain(typeOrder).range([0, plotH]).padding(0.3);

    // X axis
    const xAxisG = g.append('g').attr('transform', `translate(0,${plotH})`).call(d3.axisBottom(x).ticks(8));
    xAxisG.selectAll('text').attr('fill', textColor);
    xAxisG.selectAll('line,path').attr('stroke', gridColor);

    // Type labels on Y
    g.selectAll('text.type-label')
      .data(typeOrder.filter((t) => activeTypes.has(t)))
      .join('text')
      .attr('x', -8)
      .attr('y', (d) => yBand(d) + yBand.bandwidth() / 2)
      .attr('dy', '0.35em')
      .attr('text-anchor', 'end')
      .attr('fill', (d) => TYPE_COLORS[d] || textColor)
      .attr('font-size', 11)
      .attr('font-weight', 600)
      .text((d) => d.charAt(0).toUpperCase() + d.slice(1));

    // Horizontal lane lines
    typeOrder.forEach((t) => {
      if (!activeTypes.has(t)) return;
      const cy = yBand(t) + yBand.bandwidth() / 2;
      g.append('line')
        .attr('x1', 0).attr('x2', plotW)
        .attr('y1', cy).attr('y2', cy)
        .attr('stroke', gridColor).attr('stroke-width', 1).attr('stroke-dasharray', '4,4');
    });

    // Event dots
    const dots = g.selectAll('circle.event')
      .data(events)
      .join('circle')
      .attr('class', 'event')
      .attr('cx', (d) => x(new Date(d.timestamp)))
      .attr('cy', (d) => {
        const lane = yBand(d.event_type);
        return (lane != null ? lane : 0) + yBand.bandwidth() / 2 + (Math.random() - 0.5) * yBand.bandwidth() * 0.5;
      })
      .attr('r', 6)
      .attr('fill', (d) => TYPE_COLORS[d.event_type] || '#94a3b8')
      .attr('stroke', '#fff')
      .attr('stroke-width', 1.5)
      .style('cursor', 'pointer')
      .on('mouseover', (event, d) => {
        d3.select(event.currentTarget).attr('r', 9);
        tip().show(event,
          `<strong>${d.title || d.event_type}</strong><br/>` +
          new Date(d.timestamp).toLocaleDateString() + '<br/>' +
          (d.detail ? `<em>${d.detail.substring(0, 150)}</em><br/>` : '') +
          (d.affected_count ? `Affected: ${d.affected_count} symbols` : '')
        );
      })
      .on('mouseout', (event) => {
        d3.select(event.currentTarget).attr('r', 6);
        tip().hide();
      })
      .on('click', (_event, d) => {
        const detail = document.getElementById('memory-timeline-detail');
        if (detail) {
          detail.classList.remove('hidden');
          detail.innerHTML =
            '<div class="flex items-center justify-between mb-2">' +
            `<strong style="color:${TYPE_COLORS[d.event_type] || '#94a3b8'}">${d.title || d.event_type}</strong>` +
            '<button onclick="this.closest(\'#memory-timeline-detail\').classList.add(\'hidden\')" class="text-text-secondary hover:text-text-primary text-lg">&times;</button>' +
            '</div>' +
            `<div class="text-xs text-text-secondary mb-2">${new Date(d.timestamp).toLocaleString()}</div>` +
            (d.detail ? `<p class="text-sm mb-2">${d.detail}</p>` : '') +
            (d.affected_count ? `<div class="text-xs text-text-secondary">Affected symbols: ${d.affected_count}</div>` : '');
        }
      });

    // Zoom
    const zoom = d3.zoom().scaleExtent([0.5, 20]).on('zoom', (event) => {
      const newX = event.transform.rescaleX(x);
      xAxisG.call(d3.axisBottom(newX).ticks(8));
      xAxisG.selectAll('text').attr('fill', textColor);
      dots.attr('cx', (d) => newX(new Date(d.timestamp)));
    });
    svg.call(zoom);
  }

  let cachedData = null;

  function getActiveTypes() {
    const types = new Set();
    document.querySelectorAll('.memory-filter:checked').forEach((cb) => {
      types.add(cb.value);
    });
    return types;
  }

  function load() {
    const container = document.getElementById('memory-timeline-chart');
    if (!container) return;
    const sinceEl = document.getElementById('memory-timeline-since');
    const since = sinceEl ? sinceEl.value : '90d';
    const url = `/api/v1/memory-timeline?since=${since}`;

    fetch(url, { headers: { Accept: 'application/json' } })
      .then((r) => r.json())
      .then((json) => {
        cachedData = json.data || json;
        draw(container, cachedData, getActiveTypes());
      })
      .catch(() => empty(container, 'Failed to load memory timeline'));
  }

  window.initMemoryTimeline = function initMemoryTimeline() {
    const container = document.getElementById('memory-timeline-chart');
    if (!container) return;

    document.querySelectorAll('.memory-filter').forEach((cb) => {
      cb.addEventListener('change', () => {
        if (cachedData) {
          draw(container, cachedData, getActiveTypes());
        }
      });
    });

    const sinceEl = document.getElementById('memory-timeline-since');
    if (sinceEl) sinceEl.addEventListener('change', load);

    load();
  };
})();
