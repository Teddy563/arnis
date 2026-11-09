// Run: node tests/gui/meld-flags.mjs
// The Meld features switch off, and on with every control at its default, send no flags and
// find no problem. The defaults are read from index.html itself.
import assert from 'node:assert';
import fs from 'node:fs';
import { meldFlags, meldProblem } from '../../src/gui/js/meld-features.js';

const html = fs.readFileSync(new URL('../../src/gui/index.html', import.meta.url), 'utf8');
const state = new Map(); // id -> element, so a test can change one

function fromHtml(id) {
  const tag = html.match(new RegExp(`<(input|select)[^>]* id="${id}"[^>]*>`));
  if (!tag) return null;
  const attr = (name) => (tag[0].match(new RegExp(` ${name}="([^"]*)"`)) || [])[1];
  let value = attr('value') ?? '';
  if (tag[1] === 'select') {
    const block = html.slice(tag.index, html.indexOf('</select>', tag.index));
    const chosen = block.match(/<option value="([^"]*)" selected/) || block.match(/<option value="([^"]*)"/);
    value = chosen ? chosen[1] : '';
  }
  return { value, checked: /\bchecked\b/.test(tag[0]), closest: () => null };
}

globalThis.window = {};
globalThis.document = {
  getElementById(id) {
    if (!state.has(id)) state.set(id, fromHtml(id));
    return state.get(id);
  },
  querySelector(selector) {
    const id = selector.match(/^#([\w-]+) \.segment\.active$/)[1];
    const start = html.indexOf(`id="${id}"`);
    const block = html.slice(start, html.indexOf('</div>', start));
    const active = block.match(/class="segment active"[^>]*data-v="([^"]*)"/);
    return active ? { dataset: { v: active[1] } } : null;
  },
};

const contexts = [
  { oneWorld: false, java: true, caves: false, extendHeight: false, lods: false },
  { oneWorld: true, java: true, caves: true, extendHeight: true, lods: true },
  { oneWorld: false, java: false, caves: true, extendHeight: false, lods: false },
];
const toggle = document.getElementById('advanced-meld-features-toggle');
for (const ctx of contexts) {
  toggle.checked = false;
  assert.deepStrictEqual(meldFlags(ctx), [], 'switch off');
  assert.strictEqual(meldProblem(ctx), '', 'switch off');
  toggle.checked = true;
  assert.deepStrictEqual(meldFlags(ctx), [], 'switch on, defaults');
  assert.strictEqual(meldProblem(ctx), '', 'switch on, defaults');
}

// A field that is wrong is said in a sentence; one that is right passes.
const ctx = contexts[1];
document.getElementById('meld-seed-input').value = 'abc';
assert.match(meldProblem(ctx), /whole number/);
document.getElementById('meld-seed-input').value = '7';
assert.strictEqual(meldProblem(ctx), '');
assert.deepStrictEqual(meldFlags(ctx), ['--seed=7']);
document.getElementById('meld-min-y-input').value = '-65';
assert.match(meldProblem(ctx), /multiple of 16/);
console.log('meld-flags: ok');
