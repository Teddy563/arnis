// Run: node tests/gui/preview-popover.mjs
// The setting preview popover, with a stub document (no browser): which picture each setting and
// choice button maps to, where the popover goes, hover/focus/Escape, a missing file, and that every
// picture the page names exists and every locale has its caption.
import assert from 'node:assert';
import fs from 'node:fs';

const node = (extra = {}) => ({
  dataset: {}, listeners: {}, children: [], style: {}, textContent: '', attrs: {},
  classList: { s: new Set(), add(c) { this.s.add(c); }, remove(c) { this.s.delete(c); }, contains(c) { return this.s.has(c); } },
  addEventListener(t, f, cap) { (this.listeners[t] ||= []).push(f); },
  setAttribute(k, v) { this.attrs[k] = v; }, appendChild(c) { this.children.push(c); },
  getBoundingClientRect() { return this.rect || { left: 0, right: 0, top: 0, bottom: 0, height: 230 }; },
  ...extra,
});
const fire = (n, t, e = {}) => (n.listeners[t] || []).forEach((f) => f(e));

// A switch row (left half of a 1000 px window) and a segmented row (right half).
const sw = node({ checked: false });
const swRow = node({ rect: { left: 40, right: 480, top: 300, bottom: 360 }, querySelector: (s) => (s === 'input.switch' ? sw : null) });
const swIcon = node({ dataset: { previewOn: 'rocks-on', previewOff: 'rocks-off' }, closest: () => swRow });
const segs = ['max', 'clean', 'compact'].map((v, i) => node({ dataset: { v }, classList: node().classList }));
segs[0].classList.add('active');
const segRow = node({ rect: { left: 520, right: 960, top: 100, bottom: 160 }, querySelector: () => segs[0], querySelectorAll: () => segs });
const segIcon = node({ dataset: { previewSeg: 'road-detail' }, closest: () => segRow });
const body = node();
const doc = node({
  body, createElement: () => node(),
  querySelectorAll: () => [swIcon, segIcon],
  querySelector: () => null,
});
globalThis.document = doc;
globalThis.window = { innerWidth: 1000, innerHeight: 650, localization: { preview_rocks_on: 'Rocks: on', preview_road_detail_clean: 'Road Detail: Clean' }, addEventListener() {} };
const { previewName, captionKey, place, initPreviews } = await import('../../src/gui/js/preview-popover.js');

// names
assert.equal(previewName(swIcon, null), 'rocks-off');
sw.checked = true;
assert.equal(previewName(swIcon, null), 'rocks-on');
assert.equal(previewName(segIcon, null), 'road-detail-max', 'the active button');
assert.equal(previewName(segIcon, segs[1]), 'road-detail-clean', 'the hovered button');
assert.equal(captionKey('post-apocalyptic-on'), 'preview_post_apocalyptic_on');

// placement: beside the row, never over it
const pop = { w: 264, h: 230 };
let at = place(swRow.rect, pop, 1000, 650);
assert.equal(at.left, 488, 'left column: right of the row');
at = place(segRow.rect, pop, 1000, 650);
assert.equal(at.left, 520 - 8 - 264, 'right column: left of the row');
assert.ok(at.top >= 8 && at.top + 230 <= 642);
const wide = { left: 8, right: 992, top: 300, bottom: 360 };
at = place(wide, pop, 1000, 650);
assert.ok(at.top >= wide.bottom || at.top + pop.h <= wide.top, 'a full-width row: above or below, not over');

// behaviour
initPreviews();
const el = body.children[0];
const img = el.children[0];
assert.equal(img.loading, 'lazy');
assert.equal(img.width * 3, img.height * 4, 'fixed 4:3 box, so nothing shifts when it loads');
assert.ok(!el.classList.contains('is-visible'));
fire(swIcon, 'mouseenter');
assert.ok(el.classList.contains('is-visible'));
assert.equal(img.src, 'images/previews3d/rocks-on.webp');
assert.equal(el.children[1].textContent, 'Rocks: on');
assert.equal(img.alt, 'Rocks: on');
fire(swIcon, 'mouseleave');
assert.ok(!el.classList.contains('is-visible'));
fire(swIcon, 'focus');
assert.ok(el.classList.contains('is-visible'), 'keyboard focus shows it');
const esc = doc.listeners.keydown[0];
let stopped = false;
esc({ key: 'a', stopPropagation() { stopped = true; } });
assert.ok(el.classList.contains('is-visible') && !stopped);
esc({ key: 'Escape', stopPropagation() { stopped = true; } });
assert.ok(!el.classList.contains('is-visible') && stopped, 'Escape closes it and is not passed on to the settings page');
stopped = false;
esc({ key: 'Escape', stopPropagation() { stopped = true; } });
assert.ok(!stopped, 'with nothing open Escape goes on to close the page');
fire(segs[1], 'mouseenter');
assert.equal(img.src, 'images/previews3d/road-detail-clean.webp');
fire(segs[1], 'mouseleave');
// switching the language replaces window.localization; the next hover reads the new text
window.localization = { preview_rocks_on: 'Pietre: activat' };
fire(swIcon, 'mouseenter');
assert.equal(el.children[1].textContent, 'Pietre: activat');
assert.equal(img.alt, 'Pietre: activat');
fire(swIcon, 'mouseleave');

fire(swIcon, 'mouseenter');
img.onerror();
assert.ok(!el.classList.contains('is-visible'), 'a missing picture hides it');

// every picture the page names exists, with a caption in every locale
const html = fs.readFileSync('src/gui/index.html', 'utf8');
const names = new Set();
for (const m of html.matchAll(/data-preview-(?:on|off)="([^"]+)"/g)) names.add(m[1]);
for (const m of html.matchAll(/data-preview-seg="([^"]+)"/g)) {
  for (const b of html.matchAll(new RegExp(`id="meld-${m[1]}-group">(.*?)</div>`, 'g'))) {
    for (const v of b[1].matchAll(/data-v="([^"]+)"/g)) names.add(`${m[1]}-${v[1]}`);
  }
}
assert.ok(names.size >= 16, `${names.size} pictures named`);
const locales = fs.readdirSync('src/gui/locales').filter((f) => f.endsWith('.json') && fs.statSync(`src/gui/locales/${f}`).size > 100);
assert.ok(locales.length >= 20);
for (const n of names) {
  const f = fs.statSync(`src/gui/images/previews3d/${n}.webp`);
  assert.ok(f.size > 1000 && f.size <= 30 * 1024, `${n}.webp is ${f.size} bytes`);
  for (const l of locales) assert.ok(JSON.parse(fs.readFileSync(`src/gui/locales/${l}`, 'utf8'))[captionKey(n)], `${l} lacks ${captionKey(n)}`);
}
// Romanian: every caption present, translated (no English words left) and with its diacritics
const ro = JSON.parse(fs.readFileSync('src/gui/locales/ro.json', 'utf8'));
const en = JSON.parse(fs.readFileSync('src/gui/locales/en-US.json', 'utf8'));
for (const n of names) {
  const k = captionKey(n);
  assert.ok(typeof ro[k] === 'string' && ro[k].trim(), `ro.json lacks ${k}`);
  assert.notEqual(ro[k], en[k], `${k} is not translated in ro.json`);
  assert.ok(!/(on|off|default|density|cut|across|through|hill|river)/i.test(ro[k]), `English left in ro ${k}: ${ro[k]}`);
}
assert.ok(ro.preview_caves_on.includes('Peșteri') && ro.preview_river_bed_off.includes('râurilor'));
console.log(`preview-popover: ok (${names.size} pictures, ${locales.length} locales)`);
