// Run: node tests/gui/card-hold.mjs
// A failed piece's card (Open log) stays while the pointer is on it, on the piece, or just left
// the piece, so a neighbour's card cannot take its place on the way.
import assert from 'node:assert';

await import('../../src/gui/js/piece-style.js');
const hold = globalThis.arnisCardHold;
assert.equal(hold(true, 0, 99999), true, 'over the card');
assert.equal(hold(false, null, 99999), true, 'on the piece');
assert.equal(hold(false, 1000, 1500), true, 'just left, still travelling');
assert.equal(hold(false, 1000, 2000), false, 'left a while ago');
assert.equal(hold(false, undefined, 2000), false, 'never entered');
console.log('card-hold ok');
