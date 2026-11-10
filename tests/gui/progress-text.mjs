// Run: node tests/gui/progress-text.mjs
// An error with `detail` keeps the short message, carries the detail as title and log text; a
// Region Download fallback line becomes the translated notice.
import assert from 'node:assert';
import fs from 'node:fs';

globalThis.document = { querySelector: () => null, addEventListener() {}, getElementById: () => null };
globalThis.window = { addEventListener() {}, localization: { region_download_fallback: 'Notice' } };
const { progressText } = await import('../../src/gui/js/meld-features.js');

const full = 'Error! Could not reach the server after 3 tries: connection refused';
assert.deepEqual(progressText({ message: 'Error! Could not reach', detail: full }), { text: 'Error! Could not reach', title: full, log: full, notice: false });
assert.deepEqual(progressText({ message: 'Done!' }), { text: 'Done!', title: '', log: null, notice: false });
const n = progressText({ message: 'region_download_fallback: http_503' });
assert.equal(n.text, 'Notice');
assert.ok(n.notice);

const dir = new URL('../../src/gui/locales/', import.meta.url);
for (const f of fs.readdirSync(dir).filter((f) => f.endsWith('.json') && f !== 'en.json')) {
  assert.ok(JSON.parse(fs.readFileSync(new URL(f, dir), 'utf8')).region_download_fallback, `${f} lacks region_download_fallback`);
}
console.log('progress-text: ok');
