(function () {
  const tip = () => window.AetherTooltip || { show() {}, hide() {} };

  function empty(el, msg) {
    el.innerHTML = `<div class="chart-empty"><div class="empty-state"><div class="empty-state-title">${msg}</div></div></div>`;
  }

  function draw(container, data) {
    container.innerHTML = '';
    const series = data.velocity_series || [];
    if (!series.length) { empty(container, 'No seismograph data yet. Run a batch index to generate metrics.'); return; }

    const P = window.AetherPalette || {};
    const cyan = P.cyan || '#0ea5a8';
    const orange = P.orange || '#ea7d28';
    const danger = P.danger || '#ef4444';
    const textColor = typeof P.text === 'function' ? P.text() : '#64748b';
    const gridColor = typeof P.gridLine === 'function' ? P.gridLine() : 'rgba(203,213,225,0.5)';

    // Sort series by timestamp ascending for line chart
    const sorted = series.slice().sort((a, b) => a.batch_timestamp - b.batch_timestamp);
    const cascades = (data.cascades || []).slice().sort((a, b) => a.detected_at - b.detected_at);

    const width = container.clientWidth || 800;
    const totalHeight = 520;
    const brushHeight = 60;
    const margin = { top: 20, right: 160, bottom: brushHeight + 40, left: 55 };
    const mainHeight = totalHeight - margin.top - margin.bottom;

    const svg = d3.select(container).append('svg').attr('width', width).attr('height', totalHeight);

    // Build points
    const points = sorted.map((d) => ({
      t: new Date(d.batch_timestamp * 1000),
      velocity: d.semantic_velocity,
      shift: d.codebase_shift,
    }));

    const xExtent = d3.extent(points, (d) => d.t);
    const yMax = d3.max(points, (d) => Math.max(d.velocity, d.shift)) || 1;

    const x = d3.scaleTime().domain(xExtent).range([margin.left, width - margin.right]);
    const y = d3.scaleLinear().domain([0, yMax]).nice().range([mainHeight + margin.top, margin.top]);

    // Clip path
    svg.append('defs').append('clipPath').attr('id', 'seismo-clip')
      .append('rect').attr('x', margin.left).attr('y', margin.top)
      .attr('width', width - margin.left - margin.right).attr('height', mainHeight);

    const mainG = svg.append('g').attr('clip-path', 'url(#seismo-clip)');

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

    // Velocity line (primary, thick)
    const velocityLine = d3.line()
      .x((d) => x(d.t))
      .y((d) => y(d.velocity))
      .curve(d3.curveMonotoneX);

    mainG.append('path')
      .datum(points)
      .attr('fill', 'none')
      .attr('stroke', cyan)
      .attr('stroke-width', 2.5)
      .attr('d', velocityLine);

    // Codebase shift line (secondary, dashed)
    const shiftLine = d3.line()
      .x((d) => x(d.t))
      .y((d) => y(d.shift))
      .curve(d3.curveMonotoneX);

    mainG.append('path')
      .datum(points)
      .attr('fill', 'none')
      .attr('stroke', orange)
      .attr('stroke-width', 1.5)
      .attr('stroke-dasharray', '6,3')
      .attr('d', shiftLine);

    // Velocity dots
    mainG.selectAll('.vel-dot')
      .data(points)
      .join('circle')
      .attr('cx', (d) => x(d.t))
      .attr('cy', (d) => y(d.velocity))
      .attr('r', 3)
      .attr('fill', cyan)
      .attr('stroke', '#fff')
      .attr('stroke-width', 0.5)
      .style('cursor', 'pointer')
      .on('mouseover', (event, d) => {
        tip().show(event,
          `<strong>Semantic Velocity</strong><br/>` +
          `Velocity: ${d.velocity.toFixed(4)}<br/>` +
          `Shift: ${d.shift.toFixed(4)}<br/>` +
          d.t.toLocaleString()
        );
      })
      .on('mouseout', () => tip().hide());

    // Cascade markers (red diamonds)
    if (cascades.length) {
      const cascadePoints = cascades.map((c) => ({
        t: new Date(c.detected_at * 1000),
        delta: c.max_delta_sem,
        hops: c.total_hops,
        epicenter: c.epicenter_symbol_id,
        chain: c.chain,
      }));

      mainG.selectAll('.cascade-marker')
        .data(cascadePoints)
        .join('path')
        .attr('d', d3.symbol().type(d3.symbolDiamond).size(80))
        .attr('transform', (d) => `translate(${x(d.t)},${y(d.delta)})`)
        .attr('fill', danger)
        .attr('stroke', '#fff')
        .attr('stroke-width', 1)
        .style('cursor', 'pointer')
        .on('mouseover', (event, d) => {
          const chainLen = Array.isArray(d.chain) ? d.chain.length : 0;
          tip().show(event,
            `<strong>Cascade Event</strong><br/>` +
            `Epicenter: ${d.epicenter}<br/>` +
            `Max \u0394_sem: ${d.delta.toFixed(4)}<br/>` +
            `Hops: ${d.hops}<br/>` +
            `Chain length: ${chainLen}<br/>` +
            d.t.toLocaleString()
          );
        })
        .on('mouseout', () => tip().hide());
    }

    // Legend
    const legend = svg.append('g').attr('transform', `translate(${width - margin.right + 10},${margin.top})`);

    // Velocity legend
    const lg1 = legend.append('g');
    lg1.append('line').attr('x1', 0).attr('x2', 20).attr('y1', 6).attr('y2', 6)
      .attr('stroke', cyan).attr('stroke-width', 2.5);
    lg1.append('text').attr('x', 26).attr('y', 9).attr('fill', textColor).attr('font-size', 10)
      .text('Semantic Velocity');

    // Shift legend
    const lg2 = legend.append('g').attr('transform', 'translate(0,18)');
    lg2.append('line').attr('x1', 0).attr('x2', 20).attr('y1', 6).attr('y2', 6)
      .attr('stroke', orange).attr('stroke-width', 1.5).attr('stroke-dasharray', '6,3');
    lg2.append('text').attr('x', 26).attr('y', 9).attr('fill', textColor).attr('font-size', 10)
      .text('Codebase Shift');

    // Cascade legend
    const lg3 = legend.append('g').attr('transform', 'translate(0,36)');
    lg3.append('path').attr('d', d3.symbol().type(d3.symbolDiamond).size(50))
      .attr('transform', 'translate(10,6)').attr('fill', danger);
    lg3.append('text').attr('x', 26).attr('y', 9).attr('fill', textColor).attr('font-size', 10)
      .text('Cascade Event');

    // Brush context area
    const brushG = svg.append('g').attr('transform', `translate(0,${totalHeight - brushHeight - 10})`);
    const xBrush = d3.scaleTime().domain(xExtent).range([margin.left, width - margin.right]);
    const yBrush = d3.scaleLinear().domain(y.domain()).range([brushHeight, 0]);

    const brushVelocityLine = d3.line()
      .x((d) => xBrush(d.t))
      .y((d) => yBrush(d.velocity))
      .curve(d3.curveMonotoneX);

    brushG.append('path').datum(points)
      .attr('fill', 'none').attr('stroke', cyan)
      .attr('stroke-width', 1).attr('stroke-opacity', 0.5).attr('d', brushVelocityLine);

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
        mainG.selectAll('path').attr('d', (d) => {
          if (!Array.isArray(d)) return null;
          if (d.length && d[0].velocity !== undefined) return velocityLine(d);
          return shiftLine(d);
        });
        mainG.selectAll('circle')
          .attr('cx', (d) => d && d.t ? x(d.t) : 0)
          .attr('cy', (d) => d && d.velocity !== undefined ? y(d.velocity) : 0);
        mainG.selectAll('.cascade-marker')
          .attr('transform', (d) => d && d.t ? `translate(${x(d.t)},${y(d.delta)})` : '');
      });

    brushG.append('g').call(brush);
  }

  function load() {
    const container = document.getElementById('seismograph-timeline-chart');
    if (!container) return;
    const limitEl = document.getElementById('seismograph-timeline-limit');
    const limit = limitEl ? limitEl.value : '50';

    fetch(`/api/v1/seismograph-timeline?limit=${limit}`, { headers: { Accept: 'application/json' } })
      .then((r) => r.json())
      .then((json) => draw(container, json.data || json))
      .catch(() => empty(container, 'Failed to load seismograph data'));
  }

  window.initSeismographTimeline = function initSeismographTimeline() {
    const container = document.getElementById('seismograph-timeline-chart');
    if (!container) return;

    const limitEl = document.getElementById('seismograph-timeline-limit');
    if (limitEl) limitEl.addEventListener('change', load);

    load();
  };
})();
