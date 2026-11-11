// Run: node tests/gui/recommended.mjs
// "Use recommended" sets 8 regions and 4 workers from 220 km2 up and leaves Auto/Auto below it;
// a plan with `single` draws no cells and counts none; without the field it is unchanged.
import assert from 'node:assert';

const nodes = new Map();
const node = (id) => ({ id, value: '', checked: false, hidden: false, textContent: '', dataset: {}, children: [], classList: { toggle() {}, remove() {}, contains: () => false }, setAttribute() {}, addEventListener() {}, dispatchEvent() {}, append(...c) { this.children.push(...c); }, replaceChildren(...c) { this.children = c; }, click() { sizeChoice = this.v; } });
let sizeChoice = '';
globalThis.Event = class { constructor(type) { this.type = type; } };
globalThis.document = {
  querySelector: (q) => ({ ...node(q), v: q.match(/data-v="(\w+)"/)?.[1] }),
  addEventListener() {},
  createElement: () => node(''),
  createElementNS: () => node(''),
  getElementById: (id) => (nodes.has(id) ? nodes.get(id) : (nodes.set(id, node(id)), nodes.get(id))),
};
globalThis.window = { addEventListener() {} };
const { initRunControls, initJobPlan, schedulePlan, useRecommended, RECOMMENDED_FROM_KM2 } = await import('../../src/gui/js/meld-features.js');

const workers = document.getElementById('meld-workers-select');
assert.equal(RECOMMENDED_FROM_KM2, 220);
useRecommended(220);
assert.deepStrictEqual([workers.value, sizeChoice], ['4', '8'], '220 km2: 8 regions, 4 workers');
useRecommended(219.9);
assert.deepStrictEqual([workers.value, sizeChoice], ['auto', 'auto'], 'below: Auto/Auto');
useRecommended(NaN);
assert.deepStrictEqual([workers.value, sizeChoice], ['auto', 'auto'], 'no selection: Auto/Auto');

// A plan with and without `single`.
const sent = [];
const unit = { index: 0, size: 4, regions: 16, chunks: 1024, existing_chunks: 0, bbox: '1,2,3,4' };
const est = { workers: 1, disk_mb: [1, 2], ram_mb: [1, 2], time_min: [1, 2] };
const lines = async (plan) => {
  sent.length = 0;
  const shown = [];
  nodes.set('job-plan', { ...node('job-plan'), replaceChildren: (...c) => shown.push(c) });
  initRunControls({ key: () => `k${Math.random()}`, map: () => ({}), invoke: async () => plan, post: (m) => sent.push(m), start: async () => {}, localize: () => {} });
  initJobPlan({ changed() {}, request: () => ({ savePath: 's', worldName: 'w', bboxText: '1 2 3 4', worldScale: 1 }) });
  document.getElementById('advanced-meld-features-toggle').checked = true;
  workers.value = 'auto';
  schedulePlan(true);
  await new Promise((r) => setTimeout(r, 400));
  const text = (n) => [n.textContent, ...(n.children || []).map(text)].join('|');
  return { text: shown.flat().map(text).join('#'), outlines: sent.filter((m) => m.type === 'pieceOutlines').at(-1)?.pieces };
};
const many = await lines({ units: [unit], estimate: est });
assert.ok(many.text.includes('1 cell') && many.outlines.length === 1 && !many.text.includes('Small area'), 'absent: as before');
const off = await lines({ units: [unit], estimate: est, single: false });
assert.ok(off.text.includes('1 cell') && off.outlines.length === 1, 'false: as before');
const one = await lines({ units: [unit], estimate: est, single: true });
assert.ok(one.text.includes('Small area: built in one run.'), one.text);
assert.ok(!/cell|region|worker/.test(one.text.replace('Memory per worker', '')), `no counts: ${one.text}`);
assert.deepStrictEqual(one.outlines, [], 'no cell grid');
console.log('recommended: ok');
process.exit(0);
