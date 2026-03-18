(function () {
  'use strict';

  var checkedFiles = new Set();
  var lastContent = '';
  var rebuildTimer = null;

  // Layer colors for budget breakdown
  var layerColors = {
    sir: '#06b6d4',       // cyan
    source: '#8b5cf6',    // purple
    graph: '#f59e0b',     // amber
    coupling: '#10b981',  // emerald
    health: '#ef4444',    // red
    drift: '#f97316',     // orange
    memory: '#6366f1',    // indigo
    tests: '#84cc16',     // lime
  };

  function fetchJson(url, opts) {
    return fetch(url, opts).then(function (r) {
      if (!r.ok) throw new Error('HTTP ' + r.status);
      return r.json();
    });
  }

  function clearChildren(el) {
    while (el.firstChild) el.removeChild(el.firstChild);
  }

  function makeText(tag, cls, text) {
    var el = document.createElement(tag);
    if (cls) el.className = cls;
    if (text) el.textContent = text;
    return el;
  }

  // ── File tree rendering ────────────────────────────────────────────

  function renderTree(container, tree) {
    clearChildren(container);
    if (!tree || tree.length === 0) {
      container.appendChild(makeText('div', 'text-text-muted py-4 text-center', 'No indexed files found'));
      return;
    }
    var ul = makeText('div', 'space-y-0.5', '');
    tree.forEach(function (node) {
      ul.appendChild(renderNode(node, 0));
    });
    container.appendChild(ul);
  }

  function renderNode(node, depth) {
    var wrapper = document.createElement('div');

    if (node.type === 'directory') {
      // Directory node
      var dirRow = document.createElement('div');
      dirRow.className = 'flex items-center gap-1 py-0.5 cursor-pointer hover:bg-surface-2/40 dark:hover:bg-slate-700/30 rounded px-1';
      dirRow.style.paddingLeft = (depth * 12) + 'px';

      var toggle = makeText('span', 'text-text-muted w-3 inline-block text-center select-none', '\u25B8');

      var cb = document.createElement('input');
      cb.type = 'checkbox';
      cb.className = 'rounded border-surface-3/50 text-accent-cyan focus:ring-accent-cyan/30';
      cb.dataset.dir = node.path;

      var label = makeText('span', 'text-text-secondary hover:text-text-primary', displayName(node.path));

      dirRow.appendChild(toggle);
      dirRow.appendChild(cb);
      dirRow.appendChild(label);

      var childContainer = makeText('div', 'hidden', '');
      if (node.children) {
        node.children.forEach(function (child) {
          childContainer.appendChild(renderNode(child, depth + 1));
        });
      }

      // Toggle expand/collapse
      dirRow.addEventListener('click', function (e) {
        if (e.target === cb) return;
        var isHidden = childContainer.classList.contains('hidden');
        childContainer.classList.toggle('hidden');
        toggle.textContent = isHidden ? '\u25BE' : '\u25B8';
      });

      // Directory checkbox: check/uncheck all children
      cb.addEventListener('change', function () {
        var childBoxes = childContainer.querySelectorAll('input[type="checkbox"]');
        childBoxes.forEach(function (box) {
          box.checked = cb.checked;
          if (box.dataset.file) {
            if (cb.checked) {
              checkedFiles.add(box.dataset.file);
            } else {
              checkedFiles.delete(box.dataset.file);
            }
          }
        });
        scheduleRebuild();
      });

      wrapper.appendChild(dirRow);
      wrapper.appendChild(childContainer);

      // Auto-expand first level
      if (depth === 0) {
        childContainer.classList.remove('hidden');
        toggle.textContent = '\u25BE';
      }
    } else {
      // File node
      var fileRow = document.createElement('label');
      fileRow.className = 'flex items-center gap-1 py-0.5 cursor-pointer hover:bg-surface-2/40 dark:hover:bg-slate-700/30 rounded px-1';
      fileRow.style.paddingLeft = (depth * 12) + 'px';

      var spacer = makeText('span', 'w-3 inline-block', '');

      var fileCb = document.createElement('input');
      fileCb.type = 'checkbox';
      fileCb.className = 'rounded border-surface-3/50 text-accent-cyan focus:ring-accent-cyan/30';
      fileCb.dataset.file = node.path;

      if (checkedFiles.has(node.path)) {
        fileCb.checked = true;
      }

      var fileName = makeText('span', 'text-text-primary truncate', displayName(node.path));
      fileName.title = node.path;

      var meta = makeText('span', 'ml-auto text-text-muted flex-shrink-0', '');
      var parts = [];
      if (node.symbol_count) parts.push(node.symbol_count + ' sym');
      if (node.has_sir) parts.push('SIR');
      meta.textContent = parts.join(' \u00B7 ');

      fileCb.addEventListener('change', function () {
        if (fileCb.checked) {
          checkedFiles.add(node.path);
        } else {
          checkedFiles.delete(node.path);
        }
        scheduleRebuild();
      });

      fileRow.appendChild(spacer);
      fileRow.appendChild(fileCb);
      fileRow.appendChild(fileName);
      fileRow.appendChild(meta);
      wrapper.appendChild(fileRow);
    }

    return wrapper;
  }

  function displayName(path) {
    var parts = path.split('/');
    return parts[parts.length - 1];
  }

  // ── Context rebuild ────────────────────────────────────────────────

  function scheduleRebuild() {
    if (rebuildTimer) clearTimeout(rebuildTimer);
    rebuildTimer = setTimeout(doRebuild, 400);
  }

  function doRebuild() {
    var targets = Array.from(checkedFiles);
    if (targets.length === 0) {
      updatePreview('', { total: getBudget(), used: 0, by_layer: {} }, 0);
      return;
    }

    // Show loading
    var indicator = document.getElementById('context-loading');
    if (indicator) indicator.style.display = 'inline-block';

    var body = {
      targets: targets,
      budget: getBudget(),
      depth: getDepth(),
      layers: getLayers(),
      format: getFormat(),
    };

    var task = getTask();
    if (task) body.task = task;

    fetchJson('/api/v1/context/build', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    })
      .then(function (resp) {
        var data = resp.data || resp;
        lastContent = data.content || '';
        updatePreview(lastContent, data.budget_usage, data.target_count);
      })
      .catch(function (err) {
        var preview = document.getElementById('context-preview');
        if (preview) preview.textContent = 'Error building context: ' + err.message;
      })
      .finally(function () {
        if (indicator) indicator.style.display = '';
      });
  }

  function updatePreview(content, budgetUsage, targetCount) {
    var preview = document.getElementById('context-preview');
    if (preview) {
      clearChildren(preview);
      if (content) {
        preview.textContent = content;
      } else {
        preview.appendChild(makeText('span', 'text-text-muted', 'Select files from the tree to build context...'));
      }
    }

    // Update target count
    var countEl = document.getElementById('context-target-count');
    if (countEl) {
      countEl.textContent = targetCount ? targetCount + ' target(s)' : '';
    }

    // Update budget bar
    if (budgetUsage) {
      updateBudgetBar(budgetUsage);
      updateLayerBreakdown(budgetUsage);
    }
  }

  function updateBudgetBar(usage) {
    var total = usage.total || 1;
    var used = usage.used || 0;
    var pct = Math.min(100, Math.round((used / total) * 100));

    var label = document.getElementById('context-budget-label');
    if (label) {
      label.textContent = formatTokens(used) + ' / ' + formatTokens(total) + ' tokens (' + pct + '%)';
    }

    var fill = document.getElementById('context-budget-fill');
    if (fill) {
      fill.style.width = pct + '%';
      // Color based on usage
      fill.className = 'h-2.5 rounded-full transition-all duration-300 ';
      if (pct < 75) {
        fill.className += 'bg-accent-green';
      } else if (pct < 90) {
        fill.className += 'bg-accent-yellow';
      } else {
        fill.className += 'bg-accent-red';
      }
    }
  }

  function updateLayerBreakdown(usage) {
    var container = document.getElementById('context-layer-bars');
    if (!container) return;

    var byLayer = usage.by_layer || {};
    var keys = Object.keys(byLayer).sort();

    clearChildren(container);

    if (keys.length === 0) {
      container.appendChild(makeText('div', 'text-xs text-text-muted', 'No context built yet'));
      return;
    }

    // Segmented bar
    var barOuter = makeText('div', 'flex h-3 rounded-full overflow-hidden bg-surface-3/30 dark:bg-slate-700/50 mb-2', '');
    keys.forEach(function (key) {
      var tokens = byLayer[key];
      var pct = Math.max(1, Math.round((tokens / (usage.total || 1)) * 100));
      var color = layerColors[key] || '#94a3b8';
      var seg = document.createElement('div');
      seg.style.width = pct + '%';
      seg.style.backgroundColor = color;
      seg.title = key + ': ' + formatTokens(tokens);
      barOuter.appendChild(seg);
    });
    container.appendChild(barOuter);

    // Legend
    var legend = makeText('div', 'flex flex-wrap gap-x-3 gap-y-1', '');
    keys.forEach(function (key) {
      var tokens = byLayer[key];
      var color = layerColors[key] || '#94a3b8';

      var item = makeText('div', 'flex items-center gap-1 text-xs', '');

      var dot = document.createElement('span');
      dot.className = 'w-2 h-2 rounded-full inline-block';
      dot.style.backgroundColor = color;
      item.appendChild(dot);

      item.appendChild(makeText('span', 'text-text-secondary', key));
      item.appendChild(makeText('span', 'font-mono text-text-muted', formatTokens(tokens)));
      legend.appendChild(item);
    });
    container.appendChild(legend);
  }

  // ── Settings getters ───────────────────────────────────────────────

  function getBudget() {
    var el = document.getElementById('context-budget-slider');
    return el ? parseInt(el.value, 10) : 32000;
  }

  function getDepth() {
    var el = document.getElementById('context-depth');
    return el ? parseInt(el.value, 10) : 2;
  }

  function getFormat() {
    var el = document.getElementById('context-format');
    return el ? el.value : 'markdown';
  }

  function getTask() {
    var el = document.getElementById('context-task');
    return el && el.value.trim() ? el.value.trim() : null;
  }

  function getLayers() {
    var layers = {};
    var checkboxes = document.querySelectorAll('input[data-layer]');
    checkboxes.forEach(function (cb) {
      layers[cb.dataset.layer] = cb.checked;
    });
    return layers;
  }

  // ── Actions ────────────────────────────────────────────────────────

  function copyToClipboard() {
    if (!lastContent) return;
    navigator.clipboard.writeText(lastContent).then(function () {
      var btn = document.getElementById('context-copy-btn');
      if (btn) {
        var orig = btn.textContent;
        btn.textContent = 'Copied!';
        setTimeout(function () { btn.textContent = orig; }, 2000);
      }
    });
  }

  function exportFile() {
    if (!lastContent) return;
    var format = getFormat();
    var ext = format === 'xml' ? '.xml' : format === 'compact' ? '.txt' : '.md';
    var ts = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19);
    var filename = 'aether-context-' + ts + ext;

    var blob = new Blob([lastContent], { type: 'text/plain;charset=utf-8' });
    var url = URL.createObjectURL(blob);
    var a = document.createElement('a');
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  }

  function applyPreset(dataset) {
    if (dataset.presetBudget) {
      var slider = document.getElementById('context-budget-slider');
      if (slider) {
        slider.value = dataset.presetBudget;
        var label = document.getElementById('context-budget-slider-val');
        if (label) label.textContent = formatTokens(parseInt(dataset.presetBudget, 10));
      }
    }

    var name = dataset.presetName;
    var presetLayers = {
      quick: { sir: true, source: true, graph: false, coupling: false, health: false, drift: false, memory: false, tests: false },
      review: { sir: true, source: true, graph: true, coupling: true, health: true, drift: false, memory: false, tests: true },
      deep: { sir: true, source: true, graph: true, coupling: true, health: true, drift: true, memory: true, tests: true },
      overview: { sir: true, source: false, graph: false, coupling: false, health: true, drift: true, memory: false, tests: false },
    };
    var presetDepths = { quick: 1, review: 2, deep: 3, overview: 0 };

    if (presetLayers[name]) {
      var layerConfig = presetLayers[name];
      Object.keys(layerConfig).forEach(function (key) {
        var cb = document.getElementById('layer-' + key);
        if (cb) cb.checked = layerConfig[key];
      });
    }
    if (presetDepths[name] !== undefined) {
      var depthEl = document.getElementById('context-depth');
      if (depthEl) depthEl.value = presetDepths[name];
    }

    scheduleRebuild();
  }

  function clearAll() {
    checkedFiles.clear();
    var checkboxes = document.querySelectorAll('#context-builder-tree input[type="checkbox"]');
    checkboxes.forEach(function (cb) { cb.checked = false; });
    updatePreview('', { total: getBudget(), used: 0, by_layer: {} }, 0);
  }

  // ── Helpers ────────────────────────────────────────────────────────

  function formatTokens(n) {
    if (n >= 1000) {
      return (n / 1000).toFixed(1).replace(/\.0$/, '') + 'K';
    }
    return String(n);
  }

  // ── Init ───────────────────────────────────────────────────────────

  window.initContextBuilder = function initContextBuilder() {
    var container = document.getElementById('context-builder-tree');
    if (!container) return;

    // Reset state
    checkedFiles = new Set();
    lastContent = '';

    // Expose methods for inline onclick handlers
    window._ctxBuilder = {
      copyToClipboard: copyToClipboard,
      exportFile: exportFile,
      applyPreset: applyPreset,
      clearAll: clearAll,
    };

    // Load file tree
    fetchJson('/api/v1/context/file-tree')
      .then(function (resp) {
        var data = resp.data || resp;
        renderTree(container, data.tree);
      })
      .catch(function (err) {
        clearChildren(container);
        container.appendChild(makeText('div', 'text-accent-red py-4 text-center', 'Failed to load file tree: ' + err.message));
      });

    // Bind change events for controls that trigger rebuild
    var controls = ['context-depth', 'context-format', 'context-budget-slider'];
    controls.forEach(function (id) {
      var el = document.getElementById(id);
      if (el) {
        el.addEventListener('change', function () {
          if (id === 'context-budget-slider') {
            var label = document.getElementById('context-budget-slider-val');
            if (label) label.textContent = formatTokens(parseInt(el.value, 10));
          }
          scheduleRebuild();
        });
      }
    });

    // Task input with debounce
    var taskEl = document.getElementById('context-task');
    if (taskEl) {
      var taskTimer = null;
      taskEl.addEventListener('input', function () {
        if (taskTimer) clearTimeout(taskTimer);
        taskTimer = setTimeout(scheduleRebuild, 500);
      });
    }

    // Layer toggle change events
    var layerCbs = document.querySelectorAll('input[data-layer]');
    layerCbs.forEach(function (cb) {
      cb.addEventListener('change', scheduleRebuild);
    });
  };
})();
