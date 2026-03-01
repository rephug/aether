(function () {
  let sortField = 'risk_score';
  let sortDesc = true;

  function renderRows(rows) {
    const tbody = document.getElementById('xray-hotspots-body');
    const count = document.getElementById('xray-hotspot-count');
    if (!tbody) return;
    tbody.innerHTML = '';
    if (count) count.textContent = `${rows.length} symbols`;

    rows.forEach((row) => {
      const tr = document.createElement('tr');
      tr.className = 'clickable';
      tr.innerHTML = `
        <td><div class="font-mono text-xs">${row.qualified_name}</div><div class="text-[11px] text-text-muted">${row.file_path}</div></td>
        <td><span class="badge" style="background:${window.AetherTheme.riskColor(row.risk_score)}22;color:${window.AetherTheme.riskColor(row.risk_score)}">${Number(row.risk_score).toFixed(2)}</span></td>
        <td class="font-mono">${Number(row.pagerank || 0).toFixed(3)}</td>
        <td class="font-mono">${Number(row.drift_score || 0).toFixed(2)}</td>
        <td class="font-mono">${row.test_count ?? 0}</td>
        <td>${row.has_sir ? '<span class="badge badge-green">Yes</span>' : '<span class="badge badge-red">No</span>'}</td>
      `;
      tr.addEventListener('click', () => {
        htmx.ajax('GET', `/dashboard/frag/blast-radius?symbol_id=${row.symbol_id}`, '#main-content');
      });
      tbody.appendChild(tr);
    });
  }

  function applySort(rows) {
    const list = [...rows];
    list.sort((a, b) => {
      const av = a?.[sortField] ?? 0;
      const bv = b?.[sortField] ?? 0;
      if (typeof av === 'string' || typeof bv === 'string') {
        return sortDesc ? String(bv).localeCompare(String(av)) : String(av).localeCompare(String(bv));
      }
      return sortDesc ? Number(bv) - Number(av) : Number(av) - Number(bv);
    });
    return list;
  }

  function bindSorting(rows) {
    document.querySelectorAll('#xray-hotspots-body').forEach(() => {});
    document.querySelectorAll('th[data-sort]').forEach((th) => {
      th.addEventListener('click', () => {
        const field = th.getAttribute('data-sort');
        if (sortField === field) {
          sortDesc = !sortDesc;
        } else {
          sortField = field;
          sortDesc = true;
        }
        renderRows(applySort(rows));
      });
    });
  }

  window.initXrayHotspots = function initXrayHotspots() {
    const page = document.querySelector('[data-page="xray"]');
    if (!page) return;

    const cached = window.__AETHER_XRAY;
    if (cached && Array.isArray(cached.hotspots)) {
      const sorted = applySort(cached.hotspots);
      renderRows(sorted);
      bindSorting(cached.hotspots);
      return;
    }

    const windowVal = page.getAttribute('data-window') || '7d';
    fetch(`/api/v1/xray?window=${windowVal}`)
      .then((r) => r.json())
      .then((json) => {
        const rows = json?.data?.hotspots || [];
        const sorted = applySort(rows);
        renderRows(sorted);
        bindSorting(rows);
      })
      .catch(() => {});
  };
})();
