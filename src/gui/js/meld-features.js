// The "Meld features" switch (Settings > Advanced) and the flags it sends. Off, every
// control marked meld-only is hidden and generation gets no flags, so it runs as stock.

const el = (id) => document.getElementById(id);
const value = (id) => (el(id) ? el(id).value.trim() : '');
const choice = (id) => {
  const active = document.querySelector(`#${id} .segment.active`);
  return active ? active.dataset.v : '';
};
const on = (id) => Boolean(el(id) && el(id).checked);

export function initMeldFeatures() {
  const toggle = el('advanced-meld-features-toggle');
  if (!toggle) return;
  const apply = () => document.body.classList.toggle('meld-off', !toggle.checked);
  toggle.addEventListener('change', apply);
  apply();

  // A click only moves the active segment; settings-store.js clicks too, to restore.
  document.querySelectorAll('.segmented-control[id^="meld-"]').forEach((group) => {
    group.querySelectorAll('.segment').forEach((btn) => {
      btn.addEventListener('click', () => {
        group.querySelectorAll('.segment').forEach((b) => b.classList.toggle('active', b === btn));
      });
    });
  });
}

// The flags for one run, as the CLI takes them (`--name=value`). What a field holds is passed
// as typed: the backend parses it with the CLI's own parser and says what is wrong.
export function meldFlags({ oneWorld, java, caves, extendHeight, lods }) {
  if (!on('advanced-meld-features-toggle')) return [];
  const f = [];
  if (retryNext.length) f.push(`--only-pieces=${retryNext.join(',')}`);
  retryNext = [];
  const add = (flag, v) => v !== '' && f.push(`${flag}=${v}`);

  // Scale
  if (oneWorld) add('--one-world-workers', value('meld-workers-select'));
  const threads = value('meld-threads-input');
  if (threads) add('--threads', threads);
  else if (value('meld-cpu-target-input') !== '90') add('--cpu-target', value('meld-cpu-target-input'));
  add('--ram-budget-mb', value('meld-ram-budget-input'));

  // Generation
  add('--seed', value('meld-seed-input'));
  if (extendHeight) {
    add('--min-y', value('meld-min-y-input'));
    add('--max-y', value('meld-max-y-input'));
  }
  if (choice('meld-road-detail-group') !== 'max') add('--road-detail', choice('meld-road-detail-group'));
  if (!on('meld-buildings-toggle')) f.push('--no-buildings');
  if (choice('meld-props-group') !== 'auto') add('--props', choice('meld-props-group'));

  // Nature
  if (on('meld-rocks-toggle')) f.push('--rocks');
  if (on('meld-bushes-toggle')) f.push('--bushes');
  if (value('meld-tree-realm-select') !== 'auto') add('--tree-realm', value('meld-tree-realm-select'));
  if (value('meld-tree-pack-dir-input')) {
    add('--tree-pack-dir', value('meld-tree-pack-dir-input'));
    if (choice('meld-tree-pack-mode-group') !== 'add') add('--tree-pack-mode', choice('meld-tree-pack-mode-group'));
  }
  if (caves) {
    if (value('meld-cave-style-select') !== 'all-mix') add('--cave-style', value('meld-cave-style-select'));
    if (choice('meld-cave-ores-group') !== 'normal') add('--cave-ores', choice('meld-cave-ores-group'));
  }
  if (choice('meld-river-bed-group') !== 'off') add('--river-bed', choice('meld-river-bed-group'));
  if (choice('meld-water-detail-group') !== 'default') add('--water-detail', choice('meld-water-detail-group'));
  if (choice('meld-climate-mode-group') !== 'origin') add('--climate-mode', choice('meld-climate-mode-group'));

  // Map and World
  if (oneWorld) add('--origin', value('meld-origin-input'));
  if (java && on('meld-world-border-toggle')) f.push('--world-border');
  if (java && !oneWorld && !lods && choice('meld-region-format-group') !== 'mca') {
    add('--region-format', choice('meld-region-format-group'));
  }
  return f;
}

// ---- A job built in pieces: outlines on the map, Pause, Stop and Retry ----

// piece index -> { state, bounds }, from the job's `piece` records.
const pieces = new Map();
let paused = false;
let retryNext = [];
let call = () => {};
let postToMap = () => {};
let restart = () => {};

const outlines = () =>
  [...pieces].map(([i, p]) => ({ i, state: p.state, bounds: p.bounds }));

export function initRunControls({ invoke, post, start }) {
  call = invoke;
  postToMap = post;
  restart = start;
  const pause = el('run-pause-button');
  pause.addEventListener('click', async () => {
    paused = !paused;
    pause.textContent = paused ? 'Resume' : 'Pause';
    await call('gui_pause_generation', { paused });
  });
  el('run-stop-button').addEventListener('click', async () => {
    // A stopped queue must not stay held: the next run starts unpaused.
    paused = false;
    pause.textContent = 'Pause';
    await call('gui_stop_generation');
  });
  el('run-retry-button').addEventListener('click', () => {
    retryNext = [...pieces].filter(([, p]) => p.state === 'failed' || p.state === 'stopped').map(([i]) => i + 1);
    // Taken by the run's flags; whatever a refused start leaves is dropped.
    Promise.resolve(restart()).finally(() => { retryNext = []; });
  });
}

// A piece record from the backend. A retry keeps the pieces it does not rebuild.
export function notePiece(piece) {
  pieces.set(piece.piece, { state: piece.state, bounds: piece.bounds });
  postToMap({ type: 'pieceOutlines', pieces: outlines() });
}

export function clearPieces() {
  if (retryNext.length) return;
  pieces.clear();
  postToMap({ type: 'pieceOutlines', pieces: [] });
}

// `running` is whether a generation is under way. Pause and Stop belong to a running job,
// Retry to a finished one that left pieces behind.
export function showRunControls(running) {
  const unfinished = [...pieces.values()].some((p) => p.state === 'failed' || p.state === 'stopped');
  el('run-controls').hidden = pieces.size === 0 || !(running || unfinished);
  el('run-pause-button').hidden = el('run-stop-button').hidden = !running;
  el('run-retry-button').hidden = running || !unfinished;
  if (!running) paused = false;
  el('run-pause-button').textContent = 'Pause';
}
