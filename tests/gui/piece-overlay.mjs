// Run: node tests/gui/piece-overlay.mjs
// The piece overlay's state, with a stub window and map (no browser): what the map is sent for
// each state, the summary line, the retry of one piece, and stale pieces dropped.
import assert from 'node:assert';

const nodes = new Map();
const node = () => ({ value: '', checked: false, hidden: false, textContent: '', dataset: {}, listeners: {}, addEventListener(t, f) { this.listeners[t] = f; } });
globalThis.document = { querySelector: () => null, getElementById: (id) => (nodes.has(id) ? nodes.get(id) : (nodes.set(id, node()), nodes.get(id))) };
const winListeners = [];
globalThis.window = { addEventListener: (t, f) => winListeners.push([t, f]) };
const { notePiece, clearPieces, dropStalePieces, showRunControls, initRunControls, summary, meldFlags } =
  await import('../../src/gui/js/meld-features.js');

const sent = [];
let starts = 0;
const runs = [];
initRunControls({ invoke: async () => {}, post: (m) => sent.push(m), start: async () => { starts++; runs.push(meldFlags({}).filter((f) => f.startsWith('--only-pieces'))); }, localize: () => {} });
const last = () => sent.at(-1).pieces;
const bounds = [44.4, 26.1, 44.5, 26.2];
const rec = (piece, state, extra = {}) => notePiece({ piece, of: 4, state, bounds, ...extra });
const message = (data) => winListeners.filter(([t]) => t === 'message').forEach(([, f]) => f({ data }));
document.getElementById('advanced-meld-features-toggle').checked = true;

clearPieces('bbox-a');
for (let i = 0; i < 4; i++) rec(i, 'queued');
rec(0, 'start', { worker: 1, pct: 0 });
rec(0, 'start', { worker: 1, pct: 40 });
rec(1, 'start', { worker: 2, pct: 0 });
showRunControls(true);
assert.equal(last()[0].pct, 40);
assert.equal(last()[0].text.worker, 'Worker 1');
assert.ok(last()[0].since, 'a running piece has a clock');
assert.equal(last()[0].text.hint, '', 'no retry hint while running');
rec(0, 'done', { pct: 100 });
rec(1, 'failed', { worker: 2, error: 'out of memory' });
rec(2, 'stopped');
assert.equal(summary(), '1/4 done · 0 running · 1 failed');
assert.equal(last()[1].text.error, 'out of memory');
assert.equal(last()[1].text.state, 'Failed');
assert.ok(last()[1].end, 'a finished piece has an end');
assert.equal(last()[0].text.state, 'Done');

// While running, a click on a failed piece does nothing; afterwards it retries that piece alone.
message({ type: 'retryPiece', piece: 1 });
assert.equal(starts, 0);
showRunControls(false);
assert.equal(last()[1].text.hint, 'Click to retry this piece');
assert.equal(last()[0].text.hint, '', 'a done piece is not retried by a click');
message({ type: 'retryPiece', piece: 0 });
assert.equal(starts, 0);
message({ type: 'retryPiece', piece: 1 });
assert.equal(starts, 1);
assert.deepEqual(runs[0], ['--only-pieces=2']);
await new Promise((r) => setTimeout(r, 0));
assert.deepEqual(meldFlags({}).filter((f) => f.startsWith('--only-pieces')), [], 'the retry list is spent');

// Retry-all keeps the finished pieces and the same job.
document.getElementById('run-retry-button').listeners.click();
assert.equal(starts, 2);
assert.deepEqual(runs[1], ['--only-pieces=2,3,4']); // failed, stopped and still queued

// Another selection makes the pieces stale; the same one does not.
dropStalePieces('bbox-a');
assert.equal(last().length, 4);
dropStalePieces('bbox-b');
assert.deepEqual(last(), []);
assert.equal(document.getElementById('run-controls').hidden, true);

// The halo toggle is passed on to the map.
const halo = document.getElementById('run-halo-toggle');
halo.listeners.change({ target: { checked: true } });
assert.deepEqual(sent.at(-1), { type: 'pieceHalo', on: true });
console.log('piece-overlay: ok');
