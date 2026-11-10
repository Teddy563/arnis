// Run: node tests/gui/plural.mjs
// "1 piece" not "1 pieces": the singular string is used for 1 when the language has one.
import assert from 'node:assert';

globalThis.document = { querySelector: () => null, getElementById: () => null };
globalThis.window = { addEventListener() {}, localization: {} };
const { tn } = await import('../../src/gui/js/meld-features.js');

assert.equal(tn('plan_pieces', '{n} pieces', '{n} piece', 1), '1 piece');
assert.equal(tn('plan_pieces', '{n} pieces', '{n} piece', 30), '30 pieces');
assert.equal(tn('plan_pieces', '{n} pieces', '{n} piece', 0), '0 pieces');
window.localization = { plan_pieces: '{n} bucăți', plan_pieces_one: '{n} bucată' };
assert.equal(tn('plan_pieces', '{n} pieces', '{n} piece', 1), '1 bucată');
assert.equal(tn('plan_pieces', '{n} pieces', '{n} piece', 2), '2 bucăți');
window.localization = { plan_pieces: '{n} 个分块' }; // no singular string: the plain one
assert.equal(tn('plan_pieces', '{n} pieces', '{n} piece', 1), '1 个分块');
console.log('plural ok');
