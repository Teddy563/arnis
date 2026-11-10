// Run: node tests/gui/plural.mjs
// "1 cell" not "1 cells": the singular string is used for 1 when the language has one.
import assert from 'node:assert';

globalThis.document = { querySelector: () => null, getElementById: () => null };
globalThis.window = { addEventListener() {}, localization: {} };
const { tn } = await import('../../src/gui/js/meld-features.js');

assert.equal(tn('plan_pieces', '{n} cells', '{n} cell', 1), '1 cell');
assert.equal(tn('plan_pieces', '{n} cells', '{n} cell', 30), '30 cells');
assert.equal(tn('plan_pieces', '{n} cells', '{n} cell', 0), '0 cells');
window.localization = { plan_pieces: '{n} celule', plan_pieces_one: '{n} celulă' };
assert.equal(tn('plan_pieces', '{n} cells', '{n} cell', 1), '1 celulă');
assert.equal(tn('plan_pieces', '{n} cells', '{n} cell', 2), '2 celule');
window.localization = { plan_pieces: '{n} 个单元格' }; // no singular string: the plain one
assert.equal(tn('plan_pieces', '{n} cells', '{n} cell', 1), '1 个单元格');
window.localization = { plan_regions: '{n} regiuni', plan_regions_one: '{n} regiune' };
assert.equal(tn('plan_regions', '{n} regions', '{n} region', 1), '1 regiune');
assert.equal(tn('plan_regions', '{n} regions', '{n} region', 3), '3 regiuni');
console.log('plural ok');
