// Run: node tests/gui/projects-v2.mjs
// Projects v2: List / Grid, thumbnails, a world without any of the new fields.
import assert from 'node:assert';

const node = () => ({ children: [], listeners: {}, attrs: {}, className: '', textContent: '', setAttribute(k, v) { this.attrs[k] = v; }, addEventListener(e, f) { this.listeners[e] = f; }, append(...c) { this.children.push(...c); }, replaceChildren(...c) { this.children = c; } });
const nodes = new Map();
globalThis.document = { createElement: node, getElementById: (id) => (nodes.has(id) ? nodes.get(id) : (nodes.set(id, node()), nodes.get(id))) };
const store = new Map();
globalThis.localStorage = { setItem: (k, v) => store.set(k, v), getItem: (k) => store.get(k) ?? null };
globalThis.window = {};
const { initProjects } = await import('../../src/gui/js/meld-features.js');

const calls = [];
const worlds = [
  { name: 'Old', area_count: 1, scale: 1, locked: false, modified_ms: 1e12 },
  { name: 'Paris', area_count: 2, scale: 1, locked: false, modified_ms: 1e12, world_path: '/s/Paris', place_name: 'Paris', preview: true },
  { name: 'Bare', area_count: 1, scale: 1, locked: false, modified_ms: 1e12, world_path: '/s/Bare', preview: false },
];
const invoke = async (cmd, args) => {
  calls.push([cmd, args]);
  if (cmd === 'gui_list_one_worlds') return worlds;
  if (cmd === 'gui_world_thumbnail') return args.worldPath === '/s/Paris' ? 'data:image/png;base64,AA' : null;
};
const make = () => initProjects({ invoke, savePath: () => '/s', scale: () => 1, open: async () => {} });
const list = document.getElementById('projects-list');

// List is the default; the Grid button switches and remembers.
let refresh = make();
await refresh();
assert.strictEqual(list.className, 'projects-list');
await document.getElementById('projects-view-grid').listeners.click();
assert.strictEqual(list.className, 'projects-list is-grid');
assert.strictEqual(store.get('arnis-projects-view'), 'grid');
assert.strictEqual(document.getElementById('projects-view-grid').attrs['aria-pressed'], 'true');

// A new session starts in Grid.
make();
assert.strictEqual(list.className, 'projects-list is-grid', 'the view outlives the session');
await document.getElementById('projects-view-list').listeners.click();
assert.strictEqual(store.get('arnis-projects-view'), 'list');

// No IntersectionObserver here, so thumbnails load at once. Only a world that may have one is asked.
await new Promise((r) => setTimeout(r, 0));
const asked = calls.filter((c) => c[0] === 'gui_world_thumbnail').map((c) => c[1].worldPath);
assert.ok(asked.includes('/s/Paris') && !asked.includes('/s/Bare'), 'preview:false is never asked');
const rows = list.children;
assert.strictEqual(rows.length, 3, 'a world with no preview and no new field still renders');
assert.ok(rows[1].children[0].className === 'projects-thumb' && rows[1].children[0].children[0].src.startsWith('data:image/png'), 'a thumbnail fills its box');
assert.ok(rows[2].children[0].className.includes('is-empty'), 'no image leaves the placeholder');
assert.ok(rows[0].children[0].className.includes('is-empty'));
const title = (r) => r.children[1].children.map((c) => c.textContent);
assert.deepStrictEqual(title(rows[1]), ['Paris', 'Paris'], 'the place name sits beside the name');
assert.deepStrictEqual(title(rows[0]), ['Old'], 'an old world shows the name only');
console.log('projects-v2 ok');
