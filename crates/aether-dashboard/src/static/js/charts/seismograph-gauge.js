(function () {
  function empty(el, msg) {
    el.innerHTML = `<div class="chart-empty"><div class="empty-state"><div class="empty-state-title">${msg}</div></div></div>`;
  }

  function velocityColor(value) {
    if (value < 0.1) return '#10b981';  // green
    if (value <= 0.3) return '#f59e0b'; // amber
    return '#ef4444';                    // red
  }

  function trendArrow(trend) {
    if (trend === 'accelerating') return { arrow: '\u2191', color: '#ef4444', label: 'Accelerating' };
    if (trend === 'decelerating') return { arrow: '\u2193', color: '#3b82f6', label: 'Decelerating' };
    return { arrow: '\u2192', color: '#10b981', label: 'Stable' };
  }

  function render(container, data) {
    container.innerHTML = '';

    if (!data || data.last_batch === 0) {
      empty(container, 'No velocity data yet. Run a batch index to generate metrics.');
      return;
    }

    var color = velocityColor(data.current_velocity);
    var trend = trendArrow(data.trend);
    var lastBatch = data.last_batch > 0 ? new Date(data.last_batch * 1000).toLocaleString() : 'Never';

    var card = document.createElement('div');
    card.className = 'flex flex-col items-center gap-4 p-8 rounded-2xl border border-surface-3/50 bg-surface-1/60 max-w-md w-full';
    card.style.borderTopColor = color;
    card.style.borderTopWidth = '3px';

    // Title
    var title = document.createElement('div');
    title.className = 'text-xs uppercase tracking-widest text-text-muted font-semibold';
    title.textContent = 'Semantic Velocity';
    card.appendChild(title);

    // Big number + arrow
    var row = document.createElement('div');
    row.className = 'flex items-baseline gap-3';
    var bigNum = document.createElement('span');
    bigNum.className = 'text-6xl font-bold tabular-nums';
    bigNum.style.color = color;
    bigNum.textContent = data.current_velocity.toFixed(2);
    row.appendChild(bigNum);
    var arrowEl = document.createElement('span');
    arrowEl.className = 'text-3xl';
    arrowEl.style.color = trend.color;
    arrowEl.title = trend.label;
    arrowEl.textContent = trend.arrow;
    row.appendChild(arrowEl);
    card.appendChild(row);

    // Trend label
    var trendLabel = document.createElement('div');
    trendLabel.className = 'text-sm text-text-secondary font-medium';
    trendLabel.textContent = trend.label;
    card.appendChild(trendLabel);

    // Divider
    var div1 = document.createElement('div');
    div1.className = 'w-full border-t border-surface-3/40 my-2';
    card.appendChild(div1);

    // Stats row 1
    var grid1 = document.createElement('div');
    grid1.className = 'grid grid-cols-2 gap-4 text-center text-xs w-full';
    grid1.appendChild(makeStatCell('Codebase Shift', data.codebase_shift.toFixed(3)));
    grid1.appendChild(makeStatCell('Symbols Regenerated', String(data.symbols_regenerated)));
    card.appendChild(grid1);

    // Divider
    var div2 = document.createElement('div');
    div2.className = 'w-full border-t border-surface-3/40 my-2';
    card.appendChild(div2);

    // Stats row 2
    var grid2 = document.createElement('div');
    grid2.className = 'grid grid-cols-2 gap-4 text-center text-xs w-full';
    grid2.appendChild(makeStatCell('Previous Velocity', data.previous_velocity.toFixed(2)));
    grid2.appendChild(makeStatCell('Last Batch', lastBatch, true));
    card.appendChild(grid2);

    container.appendChild(card);
  }

  function makeStatCell(label, value, smaller) {
    var cell = document.createElement('div');
    var lbl = document.createElement('div');
    lbl.className = 'text-text-muted uppercase tracking-wider';
    lbl.textContent = label;
    cell.appendChild(lbl);
    var val = document.createElement('div');
    val.className = smaller ? 'text-sm text-text-primary mt-1' : 'text-lg font-semibold text-text-primary mt-1';
    val.textContent = value;
    cell.appendChild(val);
    return cell;
  }

  window.initSeismographGauge = function initSeismographGauge() {
    var container = document.getElementById('seismograph-gauge-container');
    if (!container) return;

    fetch('/api/v1/seismograph-gauge', { headers: { Accept: 'application/json' } })
      .then(function (r) { return r.json(); })
      .then(function (json) { render(container, json.data || json); })
      .catch(function () { empty(container, 'Failed to load velocity gauge'); });
  };
})();
