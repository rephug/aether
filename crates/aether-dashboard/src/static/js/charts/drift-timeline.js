(function () {
  const tip = () => window.AetherTooltip || { show() {}, hide() {} };

  function empty(el, msg) {
    el.innerHTML = `<div class="chart-empty"><div class="empty-state"><div class="empty-state-title">${msg}</div></div></div>`;
  }

  function draw(container, data) {
    container.innerHTML = '';
    const modules = data.modules || [];
    if (!modules.length) { empty(container, 'No drift timeline data'); return; }

    const P = window.AetherPalette || {};
    const colors = P.categorical || ['#0ea5a8', '#ea7d28', '#5e6db3', '#1b9a59', '#b7871d', '#c94a34', '#64748b', '#8b5cf6'];
    const textColor = typeof P.text === 'function' ? P.text() : '#64748b';
    const gridColor = typeof P.gridLine === 'function' ? P.gridLine() : 'rgba(203,213,225,0.5)';

    const width = container.clientWidth || 800;
    const totalHeight = 520;
    const brushHeight = 60;
    const margin = { top: 20, right: 140, bottom: brushHeight + 40, left: 50 };
    const mainHeight = totalHeight - margin.top - margin.bottom;

    const svg = d3.select(container).append('svg').attr('width', width).attr('height', totalHeight);

    // Flatten all points to get time extent
    const allPoints = [];
    modules.forEach((m) => {
      (m.series || []).forEach((p) => {
        allPoints.push({ t: new Date(p.timestamp), v: p.drift_score, module: m.name });
      });
    });
    if (!allPoints.length) { empty(container, 'No drift data points'); return; }

    const xExtent = d3.extent(allPoints, (d) => d.t);
    const x = d3.scaleTime().domain(xExtent).range([margin.left, width - margin.right]);
    const y = d3.scaleLinear()
      .domain([0, d3.max(allPoints, (d) => d.v) || 1])
      .nice()
      .range([mainHeight + margin.top, margin.top]);

    // Clip path
    svg.append('defs').append('clipPath').attr('id', 'drift-clip')
      .append('rect').attr('x', margin.left).attr('y', margin.top)
      .attr('width', width - margin.left - margin.right).attr('height', mainHeight);

    const mainG = svg.append('g').attr('clip-path', 'url(#drift-clip)');

    // X axis
    const xAxisG = svg.append('g')
      .attr('transform', `translate(0,${mainHeight + margin.top})`)
      .call(d3.axisBottom(x).ticks(6));
    xAxisG.selectAll('text').attr('fill', textColor);
    xAxisG.selectAll('line,path').attr('stroke', gridColor);

    // Y axis
    const yAxisG = svg.append('g')
      .attr('transform', `translate(${margin.left},0)`)
      .call(d3.axisLeft(y).ticks(5));
    yAxisG.selectAll('text').attr('fill', textColor);
    yAxisG.selectAll('line,path').attr('stroke', gridColor);

    // Grid lines
    svg.append('g')
      .attr('transform', `translate(${margin.left},0)`)
      .call(d3.axisLeft(y).ticks(5).tickSize(-(width - margin.left - margin.right)).tickFormat(''))
      .selectAll('line').attr('stroke', gridColor).attr('stroke-opacity', 0.3);

    // Draw lines per module
    const line = d3.line()
      .x((d) => x(new Date(d.timestamp)))
      .y((d) => y(d.drift_score))
      .curve(d3.curveMonotoneX);

    modules.forEach((mod, i) => {
      const series = mod.series || [];
      if (!series.length) return;
      const color = colors[i % colors.length];

      mainG.append('path')
        .datum(series)
        .attr('fill', 'none')
        .attr('stroke', color)
        .attr('stroke-width', 2)
        .attr('d', line);

      // Dots
      mainG.selectAll(`.dot-${i}`)
        .data(series)
        .join('circle')
        .attr('cx', (d) => x(new Date(d.timestamp)))
        .attr('cy', (d) => y(d.drift_score))
        .attr('r', 3)
        .attr('fill', color)
        .attr('stroke', '#fff')
        .attr('stroke-width', 0.5)
        .style('cursor', 'pointer')
        .on('mouseover', (event, d) => {
          tip().show(event,
            `<strong>${mod.name}</strong><br/>` +
            `Drift: ${d.drift_score.toFixed(3)}<br/>` +
            (d.symbol_name ? `Symbol: ${d.symbol_name}<br/>` : '') +
            new Date(d.timestamp).toLocaleDateString()
          );
        })
        .on('mouseout', () => tip().hide());
    });

    // Legend
    const legend = svg.append('g').attr('transform', `translate(${width - margin.right + 10},${margin.top})`);
    modules.forEach((mod, i) => {
      const lg = legend.append('g').attr('transform', `translate(0,${i * 18})`);
      lg.append('rect').attr('width', 12).attr('height', 3).attr('y', 5).attr('fill', colors[i % colors.length]);
      lg.append('text').attr('x', 16).attr('y', 9).attr('fill', textColor).attr('font-size', 10)
        .text(mod.name.length > 18 ? `…${mod.name.slice(-17)}` : mod.name);
    });

    // Brush context area
    const brushG = svg.append('g').attr('transform', `translate(0,${totalHeight - brushHeight - 10})`);
    const xBrush = d3.scaleTime().domain(xExtent).range([margin.left, width - margin.right]);
    const yBrush = d3.scaleLinear().domain(y.domain()).range([brushHeight, 0]);
    const lineBrush = d3.line()
      .x((d) => xBrush(new Date(d.timestamp)))
      .y((d) => yBrush(d.drift_score))
      .curve(d3.curveMonotoneX);

    modules.forEach((mod, i) => {
      if (!mod.series || !mod.series.length) return;
      brushG.append('path').datum(mod.series)
        .attr('fill', 'none').attr('stroke', colors[i % colors.length])
        .attr('stroke-width', 1).attr('stroke-opacity', 0.5).attr('d', lineBrush);
    });

    const brush = d3.brushX()
      .extent([[margin.left, 0], [width - margin.right, brushHeight]])
      .on('brush end', (event) => {
        if (!event.selection) {
          x.domain(xExtent);
        } else {
          x.domain(event.selection.map(xBrush.invert));
        }
        xAxisG.call(d3.axisBottom(x).ticks(6));
        xAxisG.selectAll('text').attr('fill', textColor);
        mainG.selectAll('path').attr('d', (d) => Array.isArray(d) ? line(d) : null);
        mainG.selectAll('circle')
          .attr('cx', (d) => d && d.timestamp ? x(new Date(d.timestamp)) : 0)
          .attr('cy', (d) => d && d.drift_score !== undefined ? y(d.drift_score) : 0);
      });

    brushG.append('g').call(brush);
  }

  function load() {
    const container = document.getElementById('drift-timeline-chart');
    if (!container) return;
    const topEl = document.getElementById('drift-timeline-top');
    const sinceEl = document.getElementById('drift-timeline-since');
    const top = topEl ? topEl.value : '10';
    const since = sinceEl ? sinceEl.value : '30d';
    const url = `/api/v1/drift-timeline?top=${top}&since=${since}`;

    fetch(url, { headers: { Accept: 'application/json' } })
      .then((r) => r.json())
      .then((json) => draw(container, json.data || json))
      .catch(() => empty(container, 'Failed to load drift timeline'));
  }

  window.initDriftTimeline = function initDriftTimeline() {
    const container = document.getElementById('drift-timeline-chart');
    if (!container) return;

    const topEl = document.getElementById('drift-timeline-top');
    const sinceEl = document.getElementById('drift-timeline-since');
    if (topEl) topEl.addEventListener('change', load);
    if (sinceEl) sinceEl.addEventListener('change', load);

    load();
  };
})();
