(function () {
  let timer = null;
  let currentRange = null;

  function layers() {
    const values = [];
    if (document.getElementById('layer-deps')?.checked) values.push('deps');
    if (document.getElementById('layer-drift')?.checked) values.push('drift');
    if (document.getElementById('layer-communities')?.checked) values.push('communities');
    return values.join(',');
  }

  function sliderToIso(slider, range) {
    const v = Number(slider.value || 0) / 100;
    const start = Number(range.earliest || Date.now());
    const end = Number(range.latest || Date.now());
    const ms = Math.round(start + (end - start) * v);
    return new Date(ms).toISOString();
  }

  function draw(data) {
    const container = document.getElementById('time-machine-graph');
    const log = document.getElementById('time-machine-events');
    if (!container || !log) return;

    container.innerHTML = '';
    if (!data || !Array.isArray(data.nodes)) {
      container.innerHTML = '<div class="chart-empty"><div class="empty-state-title">No time-machine data</div></div>';
      log.innerHTML = '';
      return;
    }

    currentRange = data.time_range;

    const width = container.clientWidth || 960;
    const height = Math.max(520, container.clientHeight || 520);
    const svg = d3.select(container).append('svg').attr('width', width).attr('height', height);
    const g = svg.append('g');

    const nodes = data.nodes.map((n) => ({ ...n }));
    const byId = new Map(nodes.map((n) => [n.id, n]));
    const edges = (data.edges || []).filter((e) => byId.has(e.source) && byId.has(e.target)).map((e) => ({ ...e }));

    const sim = d3.forceSimulation(nodes)
      .force('link', d3.forceLink(edges).id((d) => d.id).distance(55))
      .force('charge', d3.forceManyBody().strength(-120))
      .force('center', d3.forceCenter(width / 2, height / 2));

    const link = g.selectAll('line').data(edges).join('line').attr('stroke', '#94a3b8').attr('stroke-opacity', 0.3);
    const node = g.selectAll('circle').data(nodes).join('circle')
      .attr('r', 5)
      .attr('fill', (d) => {
        if (!document.getElementById('layer-communities')?.checked) return '#0ea5a8';
        return window.AetherTheme.communityColor(d.community_id || 0);
      })
      .attr('class', (d) => d.drift_score_at_time > 0.3 ? 'aether-pulse' : '')
      .on('click', (_e, d) => {
        const encoded = encodeURIComponent(d.id);
        htmx.ajax('GET', `/dashboard/frag/symbol/${encoded}`, {
          target: '#main-content',
          pushURL: `/dashboard/symbol/${encoded}`,
        });
      });

    sim.on('tick', () => {
      link
        .attr('x1', (d) => d.source.x)
        .attr('y1', (d) => d.source.y)
        .attr('x2', (d) => d.target.x)
        .attr('y2', (d) => d.target.y)
        .attr('display', document.getElementById('layer-deps')?.checked ? null : 'none');

      node
        .attr('cx', (d) => d.x)
        .attr('cy', (d) => d.y);
    });

    log.innerHTML = '';
    (data.events || []).forEach((ev) => {
      const item = document.createElement('button');
      item.type = 'button';
      item.className = 'w-full text-left text-xs border-b border-surface-3/20 py-1 hover:bg-surface-3/20';
      item.textContent = `[${new Date(ev.timestamp).toISOString().slice(0, 10)}] ${ev.event_type}: ${ev.qualified_name}`;
      item.addEventListener('click', () => {
        const selected = nodes.find((n) => n.id === ev.symbol_id);
        if (!selected) return;
        selected.fx = width / 2;
        selected.fy = height / 2;
        sim.alpha(0.8).restart();
        setTimeout(() => { selected.fx = null; selected.fy = null; }, 900);
      });
      log.appendChild(item);
    });
  }

  function loadForCurrentSlider() {
    const slider = document.getElementById('time-machine-at');
    if (!slider || !currentRange) return;
    const at = sliderToIso(slider, currentRange);
    fetch(`/api/v1/time-machine?at=${encodeURIComponent(at)}&layers=${encodeURIComponent(layers())}`)
      .then((r) => r.json())
      .then((json) => draw(json?.data || null))
      .catch(() => draw(null));
  }

  window.initTimeMachine = function initTimeMachine() {
    const page = document.querySelector('[data-page="time-machine"]');
    if (!page) return;

    const nowIso = new Date().toISOString();
    fetch(`/api/v1/time-machine?at=${encodeURIComponent(nowIso)}&layers=deps,drift`)
      .then((r) => r.json())
      .then((json) => {
        draw(json?.data || null);
      })
      .catch(() => draw(null));

    const slider = document.getElementById('time-machine-at');
    if (slider && !slider.dataset.bound) {
      slider.dataset.bound = '1';
      let t = null;
      slider.addEventListener('input', () => {
        clearTimeout(t);
        t = setTimeout(loadForCurrentSlider, 500);
      });
    }

    ['layer-deps', 'layer-drift', 'layer-communities'].forEach((id) => {
      const el = document.getElementById(id);
      if (el && !el.dataset.bound) {
        el.dataset.bound = '1';
        el.addEventListener('change', loadForCurrentSlider);
      }
    });

    const play = document.getElementById('time-machine-play');
    if (play && !play.dataset.bound) {
      play.dataset.bound = '1';
      play.addEventListener('click', () => {
        if (timer) {
          clearInterval(timer);
          timer = null;
          play.textContent = 'Play';
          return;
        }
        const speed = Number(document.getElementById('time-machine-speed')?.value || 1);
        play.textContent = 'Pause';
        timer = setInterval(() => {
          const slider = document.getElementById('time-machine-at');
          if (!slider) return;
          const cur = Number(slider.value || 0);
          slider.value = `${Math.min(100, cur + speed)}`;
          loadForCurrentSlider();
          if (Number(slider.value) >= 100) {
            clearInterval(timer);
            timer = null;
            play.textContent = 'Play';
          }
        }, 1000);
      });
    }
  };
})();
