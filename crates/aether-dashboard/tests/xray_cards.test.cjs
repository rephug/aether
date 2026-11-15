const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

async function render(metrics) {
  const elements = new Map();
  for (const name of Object.keys(metrics)) {
    for (const prefix of ['metric-value-', 'metric-trend-', 'sparkline-', 'metric-card-']) {
      elements.set(prefix + name, { textContent: 'old', innerHTML: 'old', style: {} });
    }
  }
  elements.set('xray-graph-coverage', { textContent: '' });
  const sandbox = {
    window: {},
    document: {
      querySelector: () => ({ getAttribute: () => '7d' }),
      getElementById: (id) => elements.get(id),
    },
    fetch: async () => ({ json: async () => ({
      data: { metrics, graph_coverage: { note: 'Observed links; completeness unknown.' } },
    }) }),
    // Unavailable and graph-only metrics must never reach status grading or D3.
    d3: new Proxy({}, { get() { throw new Error('unexpected sparkline'); } }),
  };
  sandbox.window.AetherTheme = { statusColor() { throw new Error('unexpected health grade'); } };
  vm.runInNewContext(fs.readFileSync(path.join(__dirname, '../src/static/js/charts/xray-cards.js'), 'utf8'), sandbox);
  sandbox.window.initXrayCards();
  await new Promise(setImmediate);
  return elements;
}

test('not_computed takes precedence over a numeric placeholder and removes stale charts', async () => {
  const elements = await render({
    high_coupling_pairs: { value: 0, trend: 4, sparkline: [1, 2], not_computed: true },
    graph_connectivity: { value: null, not_computed: false },
  });
  for (const name of ['high_coupling_pairs', 'graph_connectivity']) {
    assert.equal(elements.get('metric-value-' + name).textContent, 'Not computed');
    assert.equal(elements.get('metric-trend-' + name).textContent, '\u2014');
    assert.equal(elements.get('sparkline-' + name).innerHTML, '');
    assert.equal(elements.get('metric-card-' + name).style.borderColor, '#94a3b8');
  }
});

test('observed disconnection stays neutral and shows the coverage limitation', async () => {
  const elements = await render({
    orphan_count: { value: 100, trend: 0, sparkline: [1, 1], not_computed: false },
    graph_connectivity: { value: 0.01, trend: 0, sparkline: [0.01], not_computed: false },
  });
  assert.equal(elements.get('metric-value-orphan_count').textContent, '100');
  assert.equal(elements.get('metric-value-graph_connectivity').textContent, '0.01');
  for (const name of ['orphan_count', 'graph_connectivity']) {
    assert.equal(elements.get('metric-trend-' + name).textContent, 'Observed');
    assert.equal(elements.get('sparkline-' + name).innerHTML, '');
    assert.equal(elements.get('metric-card-' + name).style.borderColor, '#94a3b8');
  }
  assert.equal(elements.get('xray-graph-coverage').textContent, 'Observed links; completeness unknown.');
});
