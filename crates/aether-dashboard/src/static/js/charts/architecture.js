(function () {
  function hierarchyFrom(data, misplacedOnly) {
    const groups = new Map();
    (data.symbols || []).forEach((s) => {
      if (misplacedOnly && !s.misplaced) return;
      const c = s.community_id;
      if (!groups.has(c)) groups.set(c, { name: `community-${c}`, children: [] });
      groups.get(c).children.push({
        name: s.qualified_name,
        value: 1,
        community_id: c,
        misplaced: s.misplaced,
        file_path: s.file_path,
      });
    });
    return {
      name: 'root',
      children: Array.from(groups.values()),
    };
  }

  function draw(data, errorMessage) {
    const container = document.getElementById('architecture-treemap');
    if (!container) return;
    container.innerHTML = '';

    if (errorMessage) {
      container.innerHTML = `<div class="chart-empty"><div class="empty-state-title">${errorMessage}</div></div>`;
      return;
    }

    if (!data || data.not_computed) {
      container.innerHTML = '<div class="chart-empty"><div class="empty-state-title">Community data not computed</div></div>';
      return;
    }

    const showMisplaced = !!document.getElementById('architecture-show-misplaced')?.checked;
    const treeData = hierarchyFrom(data, showMisplaced);

    const width = container.clientWidth || 980;
    const height = Math.max(620, container.clientHeight || 620);
    const svg = d3.select(container).append('svg').attr('width', width).attr('height', height);

    const root = d3.hierarchy(treeData).sum((d) => d.value || 0.5);
    d3.treemap().tile(d3.treemapSquarify).size([width, height]).paddingInner(1)(root);

    const tip = window.AetherTooltip;

    const nodes = svg.selectAll('g.node')
      .data(root.leaves())
      .join('g')
      .attr('transform', (d) => `translate(${d.x0},${d.y0})`);

    nodes.append('rect')
      .attr('width', (d) => Math.max(0, d.x1 - d.x0))
      .attr('height', (d) => Math.max(0, d.y1 - d.y0))
      .attr('fill', (d) => {
        const c = window.AetherTheme.communityColor(d.data.community_id || 0);
        return d.data.misplaced ? d3.color(c).copy({ opacity: 0.45 }) : c;
      })
      .attr('stroke', (d) => d.data.misplaced ? '#ef4444' : '#ffffff')
      .attr('stroke-width', (d) => d.data.misplaced ? 1.4 : 0.8)
      .on('mouseover', (event, d) => {
        tip.show(event, `<strong>${d.data.name}</strong><br/>${d.data.file_path || ''}<br/>community ${d.data.community_id}${d.data.misplaced ? '<br/><em>misplaced</em>' : ''}`);
      })
      .on('mouseout', () => tip.hide());

    nodes.append('text')
      .attr('x', 4)
      .attr('y', 12)
      .attr('fill', '#0f172a')
      .attr('font-size', 10)
      .text((d) => d.data.name.split('::').pop())
      .each(function (d) {
        if ((d.x1 - d.x0) < 70 || (d.y1 - d.y0) < 20) d3.select(this).remove();
      });

    const c = document.getElementById('architecture-community-count');
    const m = document.getElementById('architecture-misplaced-count');
    if (c) c.textContent = `Neighborhoods: ${data.community_count}`;
    if (m) m.textContent = `Misplaced Components: ${data.misplaced_count}`;
  }

  window.initArchitectureMap = function initArchitectureMap() {
    const page = document.querySelector('[data-page="architecture"]');
    if (!page) return;

    const granularity = page.getAttribute('data-granularity') || 'symbol';
    fetch(`/api/v1/architecture?granularity=${encodeURIComponent(granularity)}`)
      .then(async (r) => {
        const payload = await r.json().catch(() => null);
        if (!r.ok) {
          const message = payload?.message || 'This analysis is taking too long. Try reducing the graph scope or run `aetherd health` from the CLI for faster results.';
          throw new Error(message);
        }
        return payload;
      })
      .then((json) => {
        window.__AETHER_ARCH = json?.data || null;
        draw(window.__AETHER_ARCH);
      })
      .catch((err) => draw(null, err?.message || 'Failed to load architecture data'));

    const chk = document.getElementById('architecture-show-misplaced');
    if (chk && !chk.dataset.bound) {
      chk.dataset.bound = '1';
      chk.addEventListener('change', () => draw(window.__AETHER_ARCH, null));
    }
  };
})();
