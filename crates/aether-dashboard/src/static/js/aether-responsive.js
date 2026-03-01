(function () {
  const observers = new Map();

  function debounce(fn, wait) {
    let timer = null;
    return function (...args) {
      clearTimeout(timer);
      timer = setTimeout(() => fn.apply(this, args), wait);
    };
  }

  function initResponsive(containerId, renderFn) {
    const el = document.getElementById(containerId);
    if (!el) return;

    if (observers.has(containerId)) {
      const prev = observers.get(containerId);
      prev.disconnect();
      observers.delete(containerId);
    }

    const handler = debounce(() => {
      const width = el.clientWidth || 300;
      const height = el.clientHeight || 260;
      const svg = el.querySelector('svg');
      if (svg) svg.remove();
      renderFn(width, height);
    }, 200);

    const obs = new ResizeObserver(handler);
    obs.observe(el);
    observers.set(containerId, obs);

    handler();
  }

  window.AetherResponsive = { initResponsive };
})();
