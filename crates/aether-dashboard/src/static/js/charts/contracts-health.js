(function () {
  function empty(el, msg) {
    el.textContent = '';
    var wrapper = document.createElement('div');
    wrapper.className = 'chart-empty';
    var p = document.createElement('p');
    p.className = 'text-sm text-text-secondary';
    p.textContent = msg;
    wrapper.appendChild(p);
    el.appendChild(wrapper);
  }

  function statusIcon(status) {
    if (status === 'satisfied') return '\u2705';
    if (status === 'first_violation') return '\u26a0\ufe0f';
    if (status === 'active_violation') return '\u274c';
    return '\u2753';
  }

  function statusBadgeClass(status) {
    if (status === 'satisfied') return 'bg-emerald-100 text-emerald-800 dark:bg-emerald-900/30 dark:text-emerald-300';
    if (status === 'first_violation') return 'bg-amber-100 text-amber-800 dark:bg-amber-900/30 dark:text-amber-300';
    if (status === 'active_violation') return 'bg-red-100 text-red-800 dark:bg-red-900/30 dark:text-red-300';
    return 'bg-slate-100 text-slate-600';
  }

  function formatDate(ts) {
    if (!ts) return '\u2014';
    var d = new Date(ts * 1000);
    return d.toLocaleDateString() + ' ' + d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  }

  function pct(value) {
    return (value * 100).toFixed(0) + '%';
  }

  function makeCard(value, label, colorClass) {
    var card = document.createElement('div');
    card.className = 'rounded-lg border border-surface-3/50 p-3 text-center';
    var num = document.createElement('div');
    num.className = 'text-2xl font-bold' + (colorClass ? ' ' + colorClass : '');
    num.textContent = value;
    var lbl = document.createElement('div');
    lbl.className = 'text-xs text-text-secondary';
    lbl.textContent = label;
    card.appendChild(num);
    card.appendChild(lbl);
    return card;
  }

  function makeCell(text, classes) {
    var td = document.createElement('td');
    td.className = classes || 'px-3 py-2';
    td.textContent = text;
    return td;
  }

  function makeHeaderCell(text) {
    var th = document.createElement('th');
    th.className = 'text-left px-3 py-2 font-medium';
    th.textContent = text;
    return th;
  }

  function draw(container, data) {
    container.textContent = '';
    var s = data.summary;

    // Summary cards
    var cards = document.createElement('div');
    cards.className = 'grid grid-cols-2 md:grid-cols-4 gap-3 mb-6';
    cards.appendChild(makeCard(s.total_contracts, 'Total Contracts', ''));
    cards.appendChild(makeCard(s.satisfied, 'Satisfied', 'text-emerald-600 dark:text-emerald-400'));
    cards.appendChild(makeCard(s.first_violation + s.active_violation, 'Violations', 'text-amber-600 dark:text-amber-400'));
    cards.appendChild(makeCard(pct(s.satisfaction_rate), 'Satisfaction Rate', ''));
    container.appendChild(cards);

    if (data.contracts.length === 0) {
      var noData = document.createElement('div');
      noData.className = 'rounded-lg border border-surface-3/50 p-6 text-center text-text-secondary';
      var p1 = document.createElement('p');
      p1.className = 'text-sm';
      p1.textContent = 'No intent contracts defined yet.';
      var p2 = document.createElement('p');
      p2.className = 'text-xs mt-1';
      p2.textContent = 'Create contracts via CLI: aetherd contract add <symbol> must "clause text"';
      noData.appendChild(p1);
      noData.appendChild(p2);
      container.appendChild(noData);
      return;
    }

    // Contracts table
    var section = document.createElement('div');
    section.className = 'rounded-lg border border-surface-3/50 overflow-hidden';
    var table = document.createElement('table');
    table.className = 'w-full text-sm';

    var thead = document.createElement('thead');
    thead.className = 'bg-surface-1 dark:bg-slate-800';
    var headRow = document.createElement('tr');
    ['Status', 'Symbol', 'Type', 'Clause', 'Streak', 'By'].forEach(function (h) {
      headRow.appendChild(makeHeaderCell(h));
    });
    thead.appendChild(headRow);
    table.appendChild(thead);

    var tbody = document.createElement('tbody');
    tbody.className = 'divide-y divide-surface-3/30';
    data.contracts.forEach(function (c) {
      var tr = document.createElement('tr');
      tr.className = 'hover:bg-surface-1/50 dark:hover:bg-slate-800/50';

      // Status badge
      var statusTd = document.createElement('td');
      statusTd.className = 'px-3 py-2';
      var badge = document.createElement('span');
      badge.className = 'inline-flex items-center gap-1 px-2 py-0.5 rounded-full text-xs font-medium ' + statusBadgeClass(c.status);
      badge.textContent = statusIcon(c.status) + ' ' + c.status.replace(/_/g, ' ');
      statusTd.appendChild(badge);
      tr.appendChild(statusTd);

      var symTd = makeCell(c.symbol_name, 'px-3 py-2 font-mono text-xs');
      tr.appendChild(symTd);

      var typeTd = document.createElement('td');
      typeTd.className = 'px-3 py-2';
      var typeSpan = document.createElement('span');
      typeSpan.className = 'px-1.5 py-0.5 rounded bg-surface-2 text-xs';
      typeSpan.textContent = c.clause_type;
      typeTd.appendChild(typeSpan);
      tr.appendChild(typeTd);

      tr.appendChild(makeCell(c.clause_text, 'px-3 py-2 max-w-xs truncate'));
      tr.appendChild(makeCell(String(c.violation_streak), 'px-3 py-2 text-center'));
      tr.appendChild(makeCell(c.created_by, 'px-3 py-2 text-xs text-text-secondary'));

      tbody.appendChild(tr);
    });
    table.appendChild(tbody);
    section.appendChild(table);
    container.appendChild(section);

    // Recent violations
    if (data.recent_violations.length > 0) {
      var violHeader = document.createElement('h3');
      violHeader.className = 'text-base font-semibold mt-6 mb-3';
      violHeader.textContent = 'Recent Violations';
      container.appendChild(violHeader);

      var violSection = document.createElement('div');
      violSection.className = 'rounded-lg border border-surface-3/50 overflow-hidden';
      var violTable = document.createElement('table');
      violTable.className = 'w-full text-sm';

      var violThead = document.createElement('thead');
      violThead.className = 'bg-surface-1 dark:bg-slate-800';
      var violHeadRow = document.createElement('tr');
      ['Symbol', 'Type', 'Confidence', 'Reason', 'Detected', 'Dismissed'].forEach(function (h) {
        violHeadRow.appendChild(makeHeaderCell(h));
      });
      violThead.appendChild(violHeadRow);
      violTable.appendChild(violThead);

      var violBody = document.createElement('tbody');
      violBody.className = 'divide-y divide-surface-3/30';
      data.recent_violations.forEach(function (v) {
        var tr = document.createElement('tr');
        tr.className = 'hover:bg-surface-1/50 dark:hover:bg-slate-800/50';

        tr.appendChild(makeCell(v.symbol_name, 'px-3 py-2 font-mono text-xs'));

        var vtypeTd = document.createElement('td');
        vtypeTd.className = 'px-3 py-2';
        var vtypeSpan = document.createElement('span');
        vtypeSpan.className = 'px-1.5 py-0.5 rounded bg-surface-2 text-xs';
        vtypeSpan.textContent = v.violation_type;
        vtypeTd.appendChild(vtypeSpan);
        tr.appendChild(vtypeTd);

        tr.appendChild(makeCell(v.confidence != null ? v.confidence.toFixed(2) : '\u2014', 'px-3 py-2 text-center'));
        tr.appendChild(makeCell(v.reason || '\u2014', 'px-3 py-2 max-w-xs truncate text-xs'));
        tr.appendChild(makeCell(formatDate(v.detected_at), 'px-3 py-2 text-xs'));
        tr.appendChild(makeCell(v.dismissed ? '\u2705' : '\u2014', 'px-3 py-2 text-center'));

        violBody.appendChild(tr);
      });
      violTable.appendChild(violBody);
      violSection.appendChild(violTable);
      container.appendChild(violSection);
    }
  }

  window.initContractsHealth = function initContractsHealth() {
    var container = document.getElementById('contracts-health-container');
    if (!container) return;

    fetch('/api/v1/contracts', { headers: { Accept: 'application/json' } })
      .then(function (r) { return r.json(); })
      .then(function (json) { draw(container, json.data || json); })
      .catch(function () { empty(container, 'Failed to load contract data'); });
  };
})();
