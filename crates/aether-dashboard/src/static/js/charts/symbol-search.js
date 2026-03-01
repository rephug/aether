(function () {
  const debounceTimers = new WeakMap();

  function debounce(el, fn, wait) {
    const prev = debounceTimers.get(el);
    if (prev) clearTimeout(prev);
    const t = setTimeout(fn, wait);
    debounceTimers.set(el, t);
  }

  function dispatchSelect(container, row) {
    const targetInputId = container.getAttribute('data-target-input');
    if (targetInputId) {
      const hidden = document.getElementById(targetInputId);
      if (hidden) hidden.value = row.symbol_id;
    }

    container.dispatchEvent(new CustomEvent('aether:symbol-selected', {
      bubbles: true,
      detail: {
        symbol_id: row.symbol_id,
        qualified_name: row.qualified_name,
      },
    }));
  }

  function setupContainer(container) {
    if (container.dataset.bound === '1') return;
    container.dataset.bound = '1';

    const input = container.querySelector('input[type="text"]');
    const resultBox = container.querySelector('div[id$="results"]');
    if (!input || !resultBox) return;

    let rows = [];
    let idx = -1;

    function close() {
      resultBox.classList.add('hidden');
      resultBox.innerHTML = '';
      rows = [];
      idx = -1;
    }

    function render() {
      if (!rows.length) {
        close();
        return;
      }
      resultBox.classList.remove('hidden');
      resultBox.className = 'absolute mt-1 w-full max-h-64 overflow-y-auto rounded-md border border-surface-3/50 bg-surface-1 dark:bg-slate-900 z-20';
      resultBox.innerHTML = '';

      rows.forEach((row, i) => {
        const item = document.createElement('button');
        item.type = 'button';
        item.className = `w-full text-left px-3 py-2 text-xs border-b border-surface-3/20 ${i === idx ? 'bg-surface-3/30' : 'hover:bg-surface-3/20'}`;
        item.innerHTML = `<div class="font-mono">${row.qualified_name}</div><div class="text-text-muted">${row.file_path}</div>`;
        item.addEventListener('click', () => {
          input.value = row.qualified_name;
          dispatchSelect(container, row);
          close();
        });
        resultBox.appendChild(item);
      });
    }

    input.addEventListener('input', () => {
      const q = input.value.trim();
      debounce(input, () => {
        if (!q) {
          close();
          return;
        }
        fetch(`/api/v1/search?q=${encodeURIComponent(q)}&limit=10`)
          .then((r) => r.json())
          .then((json) => {
            rows = json?.data?.results || [];
            idx = rows.length ? 0 : -1;
            render();
          })
          .catch(() => close());
      }, 300);
    });

    input.addEventListener('keydown', (e) => {
      if (!rows.length) return;
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        idx = (idx + 1) % rows.length;
        render();
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        idx = (idx - 1 + rows.length) % rows.length;
        render();
      } else if (e.key === 'Enter') {
        e.preventDefault();
        if (idx >= 0 && rows[idx]) {
          input.value = rows[idx].qualified_name;
          dispatchSelect(container, rows[idx]);
          close();
        }
      } else if (e.key === 'Escape') {
        close();
      }
    });

    document.addEventListener('click', (event) => {
      if (!container.contains(event.target)) close();
    });
  }

  window.initSymbolSearchComponents = function initSymbolSearchComponents() {
    document.querySelectorAll('[data-target-input]').forEach(setupContainer);
  };
})();
