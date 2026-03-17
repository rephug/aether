(function () {
  const tip = () => window.AetherTooltip || { show() {}, hide() {} };

  function empty(el, msg) {
    el.innerHTML = `<div class="chart-empty"><div class="empty-state"><div class="empty-state-title">${msg}</div></div></div>`;
  }

  const SIGNAL_COLORS = { structural: '#3b82f6', semantic: '#22c55e', temporal: '#f59e0b' };

  function dominantSignal(sig) {
    if (!sig) return 'structural';
    let best = 'structural';
    let max = sig.structural || 0;
    if ((sig.semantic || 0) > max) { max = sig.semantic; best = 'semantic'; }
    if ((sig.temporal || 0) > max) { best = 'temporal'; }
    return best;
  }

  function draw(container, data) {
    container.innerHTML = '';
    const modules = data.modules || [];
    const matrix = data.matrix || [];
    if (!modules.length || !matrix.length) { empty(container, 'No coupling data for chord diagram'); return; }

    const P = window.AetherPalette || {};
    const textColor = typeof P.text === 'function' ? P.text() : '#64748b';
    const colors = P.categorical || ['#0ea5a8', '#ea7d28', '#5e6db3', '#1b9a59', '#b7871d', '#c94a34', '#64748b', '#8b5cf6'];

    const width = container.clientWidth || 700;
    const height = Math.max(560, container.clientHeight || 560);
    const outerRadius = Math.min(width, height) / 2 - 60;
    const innerRadius = outerRadius - 20;

    const svg = d3.select(container).append('svg')
      .attr('width', width).attr('height', height)
      .append('g').attr('transform', `translate(${width / 2},${height / 2})`);

    const chord = d3.chord().padAngle(0.04).sortSubgroups(d3.descending);
    const chords = chord(matrix);

    const arc = d3.arc().innerRadius(innerRadius).outerRadius(outerRadius);
    const ribbon = d3.ribbon().radius(innerRadius);

    // Outer arcs (modules)
    const groupG = svg.selectAll('g.group')
      .data(chords.groups)
      .join('g')
      .attr('class', 'group');

    groupG.append('path')
      .attr('d', arc)
      .attr('fill', (d) => colors[d.index % colors.length])
      .attr('stroke', '#fff')
      .attr('stroke-width', 0.5)
      .style('cursor', 'pointer')
      .on('mouseover', (event, d) => {
        svg.selectAll('path.chord').attr('opacity', (c) =>
          c.source.index === d.index || c.target.index === d.index ? 0.85 : 0.08
        );
        tip().show(event, `<strong>${modules[d.index] || '?'}</strong>`);
      })
      .on('mouseout', () => {
        svg.selectAll('path.chord').attr('opacity', 0.65);
        tip().hide();
      });

    // Module labels
    groupG.append('text')
      .each(function (d) { d.angle = (d.startAngle + d.endAngle) / 2; })
      .attr('dy', '0.35em')
      .attr('transform', (d) =>
        `rotate(${d.angle * 180 / Math.PI - 90})translate(${outerRadius + 8})${d.angle > Math.PI ? 'rotate(180)' : ''}`
      )
      .attr('text-anchor', (d) => d.angle > Math.PI ? 'end' : null)
      .attr('fill', textColor)
      .attr('font-size', 10)
      .text((d) => {
        const name = modules[d.index] || '';
        return name.length > 20 ? `…${name.slice(-19)}` : name;
      });

    // Ribbons (chords)
    const signals = data.signal_matrix || [];
    svg.selectAll('path.chord')
      .data(chords)
      .join('path')
      .attr('class', 'chord')
      .attr('d', ribbon)
      .attr('fill', (d) => {
        const sig = signals[d.source.index] ? signals[d.source.index][d.target.index] : null;
        return SIGNAL_COLORS[dominantSignal(sig)] || '#94a3b8';
      })
      .attr('opacity', 0.65)
      .attr('stroke', 'none')
      .style('cursor', 'pointer')
      .on('mouseover', (event, d) => {
        d3.select(event.currentTarget).attr('opacity', 0.9);
        const sig = signals[d.source.index] ? signals[d.source.index][d.target.index] : {};
        tip().show(event,
          `<strong>${modules[d.source.index] || '?'}</strong> ↔ <strong>${modules[d.target.index] || '?'}</strong>` +
          `<br/>Strength: ${d.source.value.toFixed(2)}` +
          `<br/>Temporal: ${(sig?.temporal || 0).toFixed(2)}` +
          `<br/>Structural: ${(sig?.structural || 0).toFixed(2)}` +
          `<br/>Semantic: ${(sig?.semantic || 0).toFixed(2)}`
        );
        const detail = document.getElementById('coupling-chord-detail');
        if (detail) {
          detail.innerHTML =
            `<strong>${modules[d.source.index] || ''} ↔ ${modules[d.target.index] || ''}</strong><br/>` +
            '<div class="mt-2 space-y-1">' +
            `<div><span class="inline-block w-3 h-3 rounded" style="background:#f59e0b"></span> Temporal: ${(sig?.temporal || 0).toFixed(3)}</div>` +
            `<div><span class="inline-block w-3 h-3 rounded" style="background:#3b82f6"></span> Structural: ${(sig?.structural || 0).toFixed(3)}</div>` +
            `<div><span class="inline-block w-3 h-3 rounded" style="background:#22c55e"></span> Semantic: ${(sig?.semantic || 0).toFixed(3)}</div>` +
            '</div>';
        }
      })
      .on('mouseout', (event) => {
        d3.select(event.currentTarget).attr('opacity', 0.65);
        tip().hide();
      });
  }

  function load() {
    const container = document.getElementById('coupling-chord-chart');
    if (!container) return;
    const thresholdEl = document.getElementById('coupling-chord-threshold');
    const threshold = thresholdEl ? thresholdEl.value : '0.3';
    const url = `/api/v1/coupling-matrix?granularity=module&threshold=${threshold}`;

    fetch(url, { headers: { Accept: 'application/json' } })
      .then((r) => r.json())
      .then((json) => draw(container, json.data || json))
      .catch(() => empty(container, 'Failed to load coupling chord data'));
  }

  window.initCouplingChord = function initCouplingChord() {
    const container = document.getElementById('coupling-chord-chart');
    if (!container) return;

    const slider = document.getElementById('coupling-chord-threshold');
    if (slider) {
      slider.addEventListener('input', () => {
        const label = document.getElementById('coupling-chord-threshold-val');
        if (label) label.textContent = parseFloat(slider.value).toFixed(2);
        load();
      });
    }
    load();
  };
})();
