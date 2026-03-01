(function () {
  function applyResultDetails(results) {
    const byId = new Map((results || []).map((r) => [r.symbol_id, r]));
    document.querySelectorAll('#smart-search-results article[data-symbol-id]').forEach((card) => {
      const id = card.getAttribute('data-symbol-id');
      const row = byId.get(id);
      if (!row) return;

      const badges = card.querySelectorAll('.badge.badge-muted');
      if (badges.length >= 4) {
        const risk = Number(row.risk_score || 0);
        badges[0].textContent = `Risk: ${risk.toFixed(2)}`;
        badges[0].style.background = `${window.AetherTheme.riskColor(risk)}22`;
        badges[0].style.color = window.AetherTheme.riskColor(risk);
        badges[1].textContent = `PageRank: ${Number(row.pagerank || 0).toFixed(3)}`;
        badges[2].textContent = `Drift: ${Number(row.drift_score || 0).toFixed(2)}`;
        badges[3].textContent = `Tests: ${row.test_count ?? 0}`;
      }

      const related = card.querySelector('.mt-3.text-xs.text-text-muted');
      if (related) {
        const names = (row.related_symbols || []).map((s) => s.qualified_name.split('::').pop());
        related.textContent = names.length ? `Related: ${names.join(' · ')}` : 'Related: none';
      }

      const desc = card.querySelector('p.mt-3.text-xs.text-text-secondary');
      if (!desc && row.sir_summary) {
        const p = document.createElement('p');
        p.className = 'mt-3 text-xs text-text-secondary';
        p.textContent = row.sir_summary;
        card.insertBefore(p, card.querySelector('.mt-3.flex.flex-wrap.gap-2.text-xs'));
      }
    });

    const count = document.getElementById('search-result-count');
    if (count) count.textContent = `${results.length} results`;
  }

  function setupKeyboardNav() {
    const cards = Array.from(document.querySelectorAll('#smart-search-results article[data-symbol-id]'));
    if (!cards.length) return;

    let idx = 0;
    cards[idx].focus();

    function focusIndex(i) {
      idx = (i + cards.length) % cards.length;
      cards[idx].focus();
    }

    document.addEventListener('keydown', function onKey(e) {
      if (!document.querySelector('#smart-search-results')) {
        document.removeEventListener('keydown', onKey);
        return;
      }
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        focusIndex(idx + 1);
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        focusIndex(idx - 1);
      } else if (e.key === 'Enter') {
        const card = cards[idx];
        if (card) card.click();
      }
    });
  }

  window.initSmartSearch = function initSmartSearch() {
    const container = document.getElementById('smart-search-results');
    if (!container) return;

    const endpoint = container.getAttribute('data-endpoint');
    if (!endpoint) return;

    container.querySelectorAll('article').forEach((card) => {
      const skeleton = card.querySelector('.mt-3.flex.flex-wrap.gap-2.text-xs');
      if (skeleton) skeleton.style.opacity = '0.7';
    });

    fetch(endpoint)
      .then((r) => r.json())
      .then((json) => {
        applyResultDetails(json?.data?.results || []);
        setupKeyboardNav();
      })
      .catch(() => {});
  };
})();
