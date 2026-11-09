// The "Meld features" switch (Settings > Advanced) and the flags it sends. Off, every
// control marked meld-only is hidden and generation gets no flags, so it runs as stock.

const el = (id) => document.getElementById(id);
const value = (id) => (el(id) ? el(id).value.trim() : '');
const choice = (id) => {
  const active = document.querySelector(`#${id} .segment.active`);
  return active ? active.dataset.v : '';
};
let defaultArchive = '';
const on = (id) => Boolean(el(id) && el(id).checked);

// A string from the loaded language, `{name}` placeholders filled in; the English text is the
// fallback while no language is loaded.
const t = (key, fallback, vars = {}) => {
  let s = (window.localization && window.localization[key]) || fallback;
  for (const [k, v] of Object.entries(vars)) s = s.replace(`{${k}}`, v);
  return s;
};

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
  // Never for another selection or world than the pieces came from.
  if (retryNext.length && keyOf() === jobKey) f.push(`--only-pieces=${retryNext.join(',')}`);
  retryNext = [];
  const add = (flag, v) => v !== '' && f.push(`${flag}=${v}`);

  // Scale
  if (oneWorld) add('--one-world-workers', value('meld-workers-select'));
  add('--threads', value('meld-threads-input'));
  add('--ram-budget-mb', value('meld-ram-budget-input'));

  // Generation
  add('--seed', value('meld-seed-input'));
  if (extendHeight) {
    add('--min-y', value('meld-min-y-input'));
    add('--max-y', value('meld-max-y-input'));
  }
  if (choice('meld-road-detail-group') !== 'max') add('--road-detail', choice('meld-road-detail-group'));
  if (on('meld-surface-patches-toggle')) f.push('--surface-patches');
  if (!on('meld-buildings-toggle')) f.push('--no-buildings');
  if (choice('meld-props-group') !== 'auto') add('--props', choice('meld-props-group'));

  // Nature
  if (on('meld-rocks-toggle')) {
    f.push('--rocks');
    if (value('meld-rock-density-input') !== '0.02') add('--rock-density', value('meld-rock-density-input'));
  }
  if (on('meld-bushes-toggle')) {
    f.push('--bushes');
    if (value('meld-bush-density-input') !== '0.05') add('--bush-density', value('meld-bush-density-input'));
  }
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

  // Data
  const source = choice('meld-osm-source-group');
  if (source === 'archive') add('--osm-tiles-url', value('meld-archive-folder-input') || defaultArchive);
  if (source === 'region') {
    f.push('--osm-pbf=geofabrik');
    add('--osm-pbf-url', value('meld-pbf-url-input'));
  }
  add('--overpass-url', value('meld-overpass-input'));
  if (on('meld-offline-toggle')) f.push('--offline');

  // Map and World
  if (oneWorld) add('--origin', value('meld-origin-input'));
  if (java && on('meld-world-border-toggle')) f.push('--world-border');
  if (java && !oneWorld && !lods && choice('meld-region-format-group') !== 'mca') {
    add('--region-format', choice('meld-region-format-group'));
    if (value('meld-blinear-level-input') !== '6') add('--blinear-level', value('meld-blinear-level-input'));
  }
  return f;
}

// ---- A job built in pieces: outlines on the map, Pause, Stop and Retry ----

// piece index -> { state, bounds, halo, worker, pct, since, end, error }, from the job's
// `piece` records. `since` is when it began running, `end` when it stopped.
const pieces = new Map();
let paused = false;
let running = false;
// What the pieces belong to (selection and world): a different one makes them, and Retry, stale.
let jobKey = '';
let keyOf = () => '';
let retryNext = [];
let call = () => {};
let postToMap = () => {};
let restart = () => {};
let localizer = () => {};

const RUNNING = new Set(['start', 'retry']);
const STATE_TEXT = {
  queued: ['run_state_queued', 'Waiting'], start: ['run_state_running', 'Running'],
  retry: ['run_state_running', 'Running'], done: ['run_state_done', 'Done'],
  skipped: ['run_state_skipped', 'Already built'], failed: ['run_state_failed', 'Failed'],
  stopped: ['run_state_stopped', 'Stopped'],
};

// What the map draws; the words are localized here because the map has no language of its own.
const outlines = () =>
  [...pieces].map(([i, p]) => {
    const [key, fallback] = STATE_TEXT[p.state] || STATE_TEXT.queued;
    return {
      i, n: i + 1, state: p.state, bounds: p.bounds, halo: p.halo, pct: p.pct, since: p.since, end: p.end,
      text: {
        piece: t('run_piece', 'Piece {n}', { n: i + 1 }),
        state: t(key, fallback),
        worker: p.worker ? t('run_worker', 'Worker {n}', { n: p.worker }) : '',
        // The first line is the failure; the child's output after it stays in the log.
        error: (p.error || '').split('\n')[0],
        hint: canRetry(p) ? t('run_retry_hint', 'Click to retry this piece') : '',
      },
    };
  });

// A finished job's failed or stopped piece can be rebuilt alone.
const canRetry = (p) => !running && (p.state === 'failed' || p.state === 'stopped');

// "3/8 done · 2 running · 1 failed"
export function summary() {
  const all = [...pieces.values()];
  const count = (f) => all.filter(f).length;
  return t('run_summary', '{done}/{total} done · {running} running · {failed} failed', {
    done: count((p) => p.state === 'done' || p.state === 'skipped'),
    total: all.length,
    running: count((p) => RUNNING.has(p.state)),
    failed: count((p) => p.state === 'failed'),
  });
}

// One run of the job over the given pieces (1-based, as --only-pieces takes them). The run's
// flags (meldFlags) take them; one a refused start leaves behind is dropped here.
function retry(list) {
  // --only-pieces names pieces of the job the outlines came from, not of another area.
  if (keyOf() !== jobKey) return dropStalePieces();
  retryNext = list;
  Promise.resolve(restart()).finally(() => { retryNext = []; });
}

let mapWindow = () => null;

export function initRunControls({ invoke, post, start, localize, key = () => '', map = () => null }) {
  keyOf = key;
  mapWindow = map;
  call = invoke;
  localizer = localize;
  postToMap = post;
  restart = start;
  el('run-pause-button').addEventListener('click', async () => {
    setPaused(!paused);
    await call('gui_pause_generation', { paused });
  });
  el('run-stop-button').addEventListener('click', async () => {
    // A stopped queue must not stay held: the next run starts unpaused.
    setPaused(false);
    await call('gui_stop_generation');
  });
  el('run-retry-button').addEventListener('click', () => {
    retry([...pieces].filter(([, p]) => RETRYABLE.has(p.state)).map(([i]) => i + 1));
  });
  // The map: a click on a failed or stopped piece's outline retries just that piece.
  window.addEventListener('message', (event) => {
    const m = event.data;
    if (!m || m.type !== 'retryPiece' || event.source !== mapWindow()) return;
    const p = pieces.get(m.piece);
    if (p && canRetry(p)) retry([m.piece + 1]);
  });
  // The faint border around each piece showing what it builds beyond its own area.
  el('run-halo-toggle').addEventListener('change', (e) => {
    postToMap({ type: 'pieceHalo', on: e.target.checked });
  });
}

// Whether the queue is held, and what the button says: the two are one variable.
function setPaused(next) {
  paused = next;
  const button = el('run-pause-button');
  button.dataset.localize = paused ? 'run_resume' : 'run_pause';
  localizer(button, button.dataset.localize);
}

// What Retry runs: pieces that failed, were stopped, or never got to start.
const RETRYABLE = new Set(['failed', 'stopped', 'queued']);

// A piece record from the backend. A retry keeps the pieces it does not rebuild.
export function notePiece(piece) {
  const was = pieces.get(piece.piece);
  // Progress records repeat the running state: the clock starts with the first.
  const since = RUNNING.has(piece.state) ? (was && RUNNING.has(was.state) ? was.since : Date.now()) : was?.since;
  pieces.set(piece.piece, {
    state: piece.state, bounds: piece.bounds, halo: piece.halo || was?.halo,
    worker: piece.worker ?? was?.worker, since,
    end: RUNNING.has(piece.state) || piece.state === 'queued' ? undefined : (was && !RUNNING.has(was.state) && was.end) || Date.now(),
    // A piece that starts again begins at its first percent, and a failure's error is its own.
    pct: piece.pct ?? (piece.state === 'queued' ? undefined : was?.pct),
    error: piece.state === 'failed' ? piece.error : '',
  });
  postToMap({ type: 'pieceOutlines', pieces: outlines() });
}

// A new run starts clean; a retry keeps the pieces it does not rebuild. `bbox` is the selection
// the run is for.
export function clearPieces() {
  if (retryNext.length) return;
  jobKey = keyOf();
  forgetPieces();
}

// The selection changed under a finished job: its pieces (and Retry) no longer fit it.
export function dropStalePieces() {
  if (running || !pieces.size || keyOf() === jobKey) return;
  resetPieces();
}

// A new world: no piece of the old one is left on the map.
export function resetPieces() {
  if (running) return;
  forgetPieces();
  showRunControls(false);
}

function forgetPieces() {
  pieces.clear();
  postToMap({ type: 'pieceOutlines', pieces: [] });
}

// `running` is whether a generation is under way. Pause and Stop belong to a running job,
// Retry to a finished one that left pieces behind.
export function showRunControls(isRunning) {
  const wasRunning = running;
  running = isRunning;
  // The hint to click a failed piece comes and goes with the run.
  if (wasRunning !== running && pieces.size) postToMap({ type: 'pieceOutlines', pieces: outlines() });
  el('run-summary').textContent = pieces.size ? summary() : '';
  const unfinished = [...pieces.values()].some((p) => RETRYABLE.has(p.state));
  el('run-controls').hidden = pieces.size === 0 || !(running || unfinished);
  el('run-pause-button').hidden = el('run-stop-button').hidden = !running;
  el('run-retry-button').hidden = running || !unfinished;
  // A run that ends while held must not leave the backend held for the next one.
  if (!running && paused) {
    setPaused(false);
    call('gui_pause_generation', { paused: false });
  }
}

// ---- Data panel: Prepare Countries and the Download Plan ----

const mb = (b) => (b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : `${Math.max(1, Math.round(b / 1e6))} MB`);

function lines(box, rows) {
  box.replaceChildren(
    ...rows.map(([a, b]) => {
      const line = document.createElement('div');
      line.className = 'meld-data-line';
      const l = document.createElement('span');
      l.textContent = a;
      const r = document.createElement('span');
      r.textContent = b;
      line.append(l, r);
      return line;
    })
  );
  box.hidden = false;
}

// `area()` is the selected bbox; `plan()` the settings a run's plan depends on.
export function initDataPanel({ invoke, area, plan }) {
  const needArea = () => t('meld_select_area', 'Select an area on the map first.');
  const list = el('meld-prepare-list');
  const out = el('meld-plan-out');
  const folder = () => value('meld-archive-folder-input');
  const say = (box, text) => lines(box, [[text, '']]);
  invoke('gui_default_archive_folder')
    .then((dir) => { defaultArchive = dir || ''; })
    .catch(() => {});

  el('meld-prepare-check').addEventListener('click', async () => {
    if (!area()) return say(list, needArea());
    say(list, t('meld_asking_geofabrik', 'Asking Geofabrik...'));
    try {
      const p = await invoke('gui_prepare_plan', { bboxText: area(), folder: folder() });
      const rows = p.extracts.map((e) => [e.baked ? t('meld_baked', '{name} (baked)', { name: e.name }) : e.name, t('meld_extract_sizes', '{download} download, {archive} archive', { download: mb(e.bytes), archive: mb(e.archive_bytes) })]);
      rows.push([t('meld_archive_folder_line', 'Archive folder'), p.folder]);
      if (p.free_bytes != null) rows.push([t('meld_free_peak', 'Free space / peak during the bake'), `${mb(p.free_bytes)} / ${mb(p.peak_bytes)}`]);
      if (p.uncovered_points) rows.push([t('meld_uncovered', 'Not covered by any country (open sea)'), t('meld_points', '{n} points', { n: p.uncovered_points })]);
      lines(list, rows);
    } catch (e) {
      say(list, String(e));
    }
  });

  const stop = el('meld-prepare-stop');
  el('meld-prepare-bake').addEventListener('click', async () => {
    if (!area()) return say(list, needArea());
    stop.hidden = false;
    say(list, t('meld_baking', 'Baking: the progress is on the main bar.'));
    try {
      const done = await invoke('gui_bake_archive', {
        bboxText: area(), folder: folder(), flags: plan().flags,
      });
      say(list, done ? t('meld_bake_done', 'The local archive is ready. Pick Local Archive as the OSM Data Source to use it.') : t('meld_bake_stopped', 'Stopped. Finished countries are kept.'));
    } catch (e) {
      say(list, String(e));
    }
    stop.hidden = true;
  });
  stop.addEventListener('click', () => invoke('gui_cancel_bake'));

  el('meld-plan-show').addEventListener('click', async () => {
    if (!area()) return say(out, needArea());
    try {
      const p = await invoke('gui_data_plan', { bboxText: area(), ...plan() });
      const rows = p.items.map((i) => [t('meld_cached', '{source}: {cached} of {total} cached', { source: i.source, cached: i.cached, total: i.total }), i.missing_bytes != null ? t('meld_to_download', '{size} to download', { size: mb(i.missing_bytes) }) : t('meld_size_unknown', 'size unknown')]);
      if (p.free_bytes != null) rows.push([t('meld_free_space', 'Free space'), mb(p.free_bytes)]);
      lines(out, rows.length ? rows : [[t('meld_nothing_to_download', 'Nothing to download.'), '']]);
    } catch (e) {
      say(out, String(e));
    }
  });
}
