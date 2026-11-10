// Run: node tests/gui/plan-summary.mjs
// The plan headline: time and size on one line each, the disk line fits / does not fit, and
// the longest strings still fit the panel (there is no DOM here: a character-width model
// stands in for scrollWidth <= clientWidth; the real window was measured at 1000x650 and 1920x1080).
import assert from 'node:assert';

const node = () => ({ value: '', hidden: false, textContent: '', className: '', children: [], classList: { toggle() {}, remove() {} }, setAttribute() {}, addEventListener() {}, append(...c) { this.children.push(...c); } });
globalThis.document = { querySelector: () => null, addEventListener() {}, getElementById: () => null, createElement: node };
globalThis.window = { addEventListener() {} };
const { planHead } = await import('../../src/gui/js/meld-features.js');

const huge = { disk_mb: [3.8 * 1048576, 4.6 * 1048576], ram_mb: [552, 704], time_min: [47 * 60 + 50, 139 * 60 + 10], workers: 4 };
const stats = (e) => planHead(e, []).at(0).children.filter((c) => c.className.startsWith('plan-stat')).map((c) => c.textContent);
assert.deepStrictEqual(stats(huge), ['≈ 48–139 h', '3.8–4.6 TB'], 'long jobs in hours, size as one range');
assert.deepStrictEqual(stats({ ...huge, time_min: [3, 40], disk_mb: [300, 450] }), ['≈ 3–40 min', '300–450 MB']);

// Disk line both ways.
const disk = (e) => planHead(e, []).at(1);
const no = disk({ ...huge, free_disk_mb: 48.1 * 1024 });
assert.ok(no.className.includes('is-low') && !no.hidden, 'does not fit: red line');
assert.strictEqual(no.textContent, 'Needs 3.8–4.6 TB, 48.1 GB free on this disk');
const yes = disk({ ...huge, disk_mb: [120 * 1024, 150 * 1024], free_disk_mb: 412 * 1024 });
assert.ok(!yes.className.includes('is-low') && !yes.hidden);
assert.strictEqual(yes.textContent, 'Fits: 412.0 GB free');
assert.ok(disk(huge).hidden, 'free space unknown: no line');

// Fit model: bold 20px digits ~12.5px, 16px icon + 6px gap + 2 x 22px padding. The panel is
// 250px wide at the narrowest (1000x650), so the headline may use 164px: 13 characters.
for (const x of stats(huge)) assert.ok(x.length * 12.5 <= 164, `${x} fits on one line`);
console.log('plan-summary: ok');
process.exit(0);
