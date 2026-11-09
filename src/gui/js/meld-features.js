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
  // The map-first page (styles.css, body.map-first) with Meld features on, the 3.4.0-style one
  // off when Main Page Layout is Auto; Map first is the default for everyone.
  const layout = el('layout-select');
  const about = el('layout-about-select');
  const apply = () => {
    const mode = layout ? layout.value : 'map';
    document.body.classList.toggle('meld-off', !toggle.checked);
    const mapFirst = mode === 'map' || (mode === 'auto' && toggle.checked);
    document.body.classList.toggle('map-first', mapFirst);
    if (about) about.value = mapFirst ? 'map' : 'classic';
    const dock = el('preview3d-mini');
    if (dock) {
      let w = 0;
      try { w = Number(localStorage.getItem('arnis-preview-dock-width')); } catch (_) { /* default size */ }
      dock.style.width = mapFirst && w >= 200 ? `${w}px` : '';
    }
    // Off, a finished job's pieces leave the map with the rest of Meld features.
    if (!toggle.checked && !running && pieces.size) resetPieces();
  };
  toggle.addEventListener('change', apply);
  if (layout) layout.addEventListener('change', apply);
  // Settings > About sets the same setting (and so its storage).
  if (about && layout) {
    about.addEventListener('change', () => {
      layout.value = about.value;
      layout.dispatchEvent(new Event('change', { bubbles: true }));
    });
  }
  apply();

  // The docked 3D preview keeps the width it was dragged to (map-first page only: the classic
  // page sizes it itself, so apply() takes the dragged width off there).
  const dock = el('preview3d-mini');
  if (dock && window.ResizeObserver) {
    new ResizeObserver(() => {
      if (!document.body.classList.contains('map-first') || !dock.offsetWidth) return;
      try { localStorage.setItem('arnis-preview-dock-width', String(dock.offsetWidth)); } catch (_) { /* not kept */ }
    }).observe(dock);
  }

  // A click only moves the active segment; settings-store.js clicks too, to restore.
  document.querySelectorAll('.segmented-control[id^="meld-"]').forEach((group) => {
    group.querySelectorAll('.segment').forEach((btn) => {
      btn.addEventListener('click', () => {
        group.querySelectorAll('.segment').forEach((b) => b.classList.toggle('active', b === btn));
      });
    });
  });
}

// What is wrong with a field, in a short sentence, or ''. The backend parses the same values
// with the CLI's own parser, but its message is cut to a few words in the status line.
export function meldProblem({ oneWorld, java, extendHeight }) {
  if (!on('advanced-meld-features-toggle')) return '';
  const name = (id) => {
    const row = el(id).closest('.settings-row');
    const label = row && row.querySelector('.setting-heading > span');
    return label ? label.textContent : id;
  };
  const whole = (id, min, max, rule) => {
    const v = value(id);
    if (v === '') return '';
    if (!/^-?\d+$/.test(v)) return t('meld_err_whole', '{name} must be a whole number.', { name: name(id) });
    const n = Number(v);
    const vars = { name: name(id), min, max };
    if (n < min || n > max) return t('meld_err_range', '{name} must be from {min} to {max}.', vars);
    if (rule === 'floor' && n % 16 !== 0) return t('meld_err_multiple', '{name} must be a multiple of 16 from {min} to {max}.', vars);
    if (rule === 'top' && ((n % 16) + 16) % 16 !== 15) return t('meld_err_section_end', '{name} must be one less than a multiple of 16, from {min} to {max}.', vars);
    return '';
  };
  const share = (id) => {
    const v = value(id);
    return v === '' || (Number.isFinite(Number(v)) && Number(v) >= 0 && Number(v) <= 1)
      ? '' : t('meld_err_share', '{name} must be a number from 0 to 1.', { name: name(id) });
  };
  const problems = [
    whole('meld-threads-input', 1, 4096),
    whole('meld-ram-budget-input', 1, 100000000),
    whole('meld-cpu-target-input', 10, 100),
    whole('meld-margin-input', 0, 4),
    whole('meld-seed-input', 0, Number.MAX_SAFE_INTEGER),
    extendHeight ? whole('meld-min-y-input', -2032, -64, 'floor') : '',
    extendHeight ? whole('meld-max-y-input', 319, 2031, 'top') : '',
    share('meld-rock-density-input'),
    share('meld-bush-density-input'),
    whole('meld-blinear-level-input', 1, 22),
  ];
  const origin = value('meld-origin-input');
  if (oneWorld && origin) {
    const [lat, lon, ...rest] = origin.split(',').map((p) => Number(p.trim()));
    if (rest.length || !(Math.abs(lat) <= 85) || !(Math.abs(lon) <= 180) || origin.split(',').length !== 2) {
      problems.push(t('meld_err_origin', '{name} must be LAT,LON, such as 44.44,26.10.', { name: name('meld-origin-input') }));
    }
  }
  return problems.find(Boolean) || '';
}

// The flags for one run, as the CLI takes them (`--name=value`). What a field holds is passed
// as typed: the backend parses it with the CLI's own parser and says what is wrong.
// `plan` is for the plan drawn before a run: it leaves a pending retry for the run itself.
export function meldFlags({ oneWorld, java, caves, extendHeight, lods, plan = false }) {
  if (!on('advanced-meld-features-toggle')) return [];
  const f = [];
  // Never for another selection or world than the pieces came from.
  if (!plan) {
    if (retryNext.length && keyOf() === jobKey) f.push(`--only-pieces=${retryNext.join(',')}`);
    retryNext = [];
  }
  const add = (flag, v) => v !== '' && f.push(`${flag}=${v}`);

  // Scale
  if (oneWorld) add('--one-world-workers', value('meld-workers-select'));
  // How the job is cut and built: only a job in pieces has them.
  if (oneWorld && value('meld-workers-select')) {
    if (choice('meld-snap-group') !== 'off') add('--snap-regions', choice('meld-snap-group'));
    if (value('meld-margin-input') !== '0') add('--margin-regions', value('meld-margin-input'));
    if (choice('meld-piece-size-group') !== '4') add('--unit-regions', choice('meld-piece-size-group'));
    if (value('meld-piece-order-select') !== 'centre') add('--piece-order', value('meld-piece-order-select'));
    if (on('meld-skip-built-toggle')) f.push('--skip-built');
    if (on('meld-stitch-map-toggle')) f.push('--stitch-map');
  }
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
        size: sizeText(i),
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

// When this run started: the ETA counts only what it built.
let runStart = 0;

// The shown ETA in minutes, smoothed as 3.4.0's was: it falls gently and resists rising.
let shownEta = null;
const ETA_A_DOWN = 0.28;
const ETA_A_UP = 0.1;

// " · 3:12 · about 12 min left": time since this run started, then the rate of this run so far
// (its finished pieces and the running ones' share) carried over what is left. No ETA until a
// piece has finished.
function eta() {
  if (!running) return '';
  const ms = Date.now() - runStart;
  const s = Math.floor(ms / 1000);
  let out = ` · ${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  const all = [...pieces.values()];
  const built = all.filter((p) => p.state === 'done' && p.end >= runStart).length;
  const share = all.filter((p) => RUNNING.has(p.state)).reduce((sum, p) => sum + (p.pct || 0) / 100, 0);
  const left = all.filter((p) => p.state === 'queued' || RUNNING.has(p.state)).length - share;
  if (!built || left <= 0) return out;
  const raw = (ms / (built + share)) * left / 60000;
  shownEta = shownEta == null ? raw : shownEta + (raw <= shownEta ? ETA_A_DOWN : ETA_A_UP) * (raw - shownEta);
  return out + ' · ' + t('run_eta', 'about {time} left', { time: minutes(shownEta) });
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
  // The run line ticks once a second while a job runs.
  setInterval(() => {
    if (running && pieces.size) el('run-summary').textContent = summary() + eta();
  }, 1000);
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
  runStart = Date.now();
  shownEta = null;
  if (retryNext.length) return;
  jobKey = keyOf();
  forgetPieces();
}

// The selection changed under a finished job: its pieces (and Retry) no longer fit it.
export function dropStalePieces() {
  schedulePlan();
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
  el('run-summary').textContent = pieces.size ? summary() + eta() : '';
  // A run that starts puts its own line in place of the plan; one that ended built pieces,
  // so the plan's states are read again.
  if (!wasRunning && running) schedulePlan();
  if (wasRunning && !running) schedulePlan(true);
  // Expand is for between runs.
  el('plan-expand-button').classList.toggle('is-running', running);
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

// ---- The plan of a job in pieces, drawn before it runs ----

// `request()` is what the plan is for ({ savePath, worldName, bboxText, worldScale }), or null
// when no job in pieces would run.
let planRequest = () => null;
let planTimer = 0;
let planKey = '';
// Only the newest answer is drawn.
let planSeq = 0;
// Past this many pieces the map would crawl; the totals still show.
const MAX_DRAWN = 3000;

// The last plan drawn, { n pieces, w workers }, while it is the current one; null otherwise.
let lastPlan = null;
export const planPieces = () => lastPlan;
// Whether Start would run a job in pieces (it asks first: main.js, the hold to confirm).
export const isPiecesJob = () => Boolean(planWanted().req);
let onPlan = () => {};

export function initJobPlan({ request, changed = () => {} }) {
  planRequest = request;
  onPlan = changed;
  // Any setting may change the plan; an unchanged request is not asked again.
  ['change', 'click'].forEach((type) => document.addEventListener(type, () => schedulePlan()));
  schedulePlan();
}

// What the plan would be asked for now, and its key; null when no job in pieces would run.
function planWanted() {
  const req = on('advanced-meld-features-toggle') && !running ? planRequest() : null;
  const flags = req ? meldFlags({ oneWorld: true, java: true, plan: true }) : [];
  return { req, flags, key: req ? JSON.stringify([req, flags]) : '' };
}

// `fresh` asks again even for an unchanged request: the world under it changed. A changed
// request clears the old plan from the map at once and drops any answer still on its way.
export function schedulePlan(fresh = false) {
  if (fresh) planKey = '';
  if (planWanted().key !== planKey) {
    planSeq++;
    // A running job keeps its "Builds in n pieces" line.
    if (lastPlan && !running) {
      lastPlan = null;
      onPlan();
    }
    if (!pieces.size) postToMap({ type: 'pieceOutlines', pieces: [] });
  }
  clearTimeout(planTimer);
  planTimer = setTimeout(updatePlan, 250);
}

// Selects `bounds` ([south, west, north, east]) with a ring of regions around it: a job in
// pieces with a margin of at least one region. `skipBuilt` leaves out what the world has.
export function selectWithRing(bounds, skipBuilt = false) {
  const set = (id, prop, v) => {
    if (!el(id) || el(id)[prop] === v) return;
    el(id)[prop] = v;
    el(id).dispatchEvent(new Event('change', { bubbles: true }));
  };
  if (!value('meld-workers-select')) set('meld-workers-select', 'value', 'auto');
  if (value('meld-margin-input') === '0') set('meld-margin-input', 'value', '1');
  if (skipBuilt) set('meld-skip-built-toggle', 'checked', true);
  postToMap({ type: 'setBbox', bounds });
}

const gb = (mbs) => (mbs >= 1048576 ? `${(mbs / 1048576).toFixed(1)} TB` : mbs >= 1024 ? `${(mbs / 1024).toFixed(1)} GB` : `${Math.round(mbs)} MB`);
// "327–388 MB": a unit both ends share is said once.
const range = ([lo, hi], unit) => {
  const [a, b] = [unit(lo), unit(hi)];
  if (a === b) return a;
  const u = a.match(/^[\d.,]+( \S+)$/);
  return u && b.endsWith(u[1]) ? `${a.slice(0, -u[1].length)}–${b}` : `${a}–${b}`;
};
// Rounded so a time never claims more than it knows (as 3.4.0's formatEstimateTime): 5 s
// steps, then minutes, then 10 minutes.
const minutes = (m) => {
  if (m < 1.5) return t('plan_seconds', '{s} s', { s: Math.max(5, Math.round((m * 60) / 5) * 5) });
  if (m < 60) return t('plan_minutes', '{m} min', { m: Math.round(m) });
  const r = Math.round(m / 10) * 10;
  return t('plan_hours', '{h} h {m} min', { h: Math.floor(r / 60), m: r % 60 });
};

// "4 × 4 regions" for the piece numbered `i` in the last plan.
const planSizes = new Map();
const sizeText = (i) => (planSizes.has(i) ? t('plan_piece_size', '{n} × {m} regions', { n: planSizes.get(i), m: planSizes.get(i) }) : '');

const div = (className, textContent) => Object.assign(document.createElement('div'), { className, textContent });

// The plan's blocks, a warning, and the legend of what the map shows: the selection, the
// pieces to build and, with a margin, the ring around it.
function showPlan(rows, warn = '', ring = false) {
  const box = el('job-plan');
  if (!box) return;
  box.replaceChildren(...rows);
  if (warn) box.append(div('job-plan-warn', warn));
  if (rows.length > 1) {
    const legend = div('job-plan-legend', '');
    const keys = [['selection', 'plan_legend_selection', 'Selection'], ['pieces', 'plan_legend_pieces', 'Pieces to build']];
    if (ring) keys.push(['ring', 'plan_legend_ring', 'Ring (margin)']);
    for (const [kind, key, fallback] of keys) {
      const item = document.createElement('span');
      item.append(Object.assign(document.createElement('i'), { className: `swatch swatch-${kind}` }), t(key, fallback));
      legend.append(item);
    }
    box.append(legend);
  }
  box.hidden = !rows.length;
}

// The Start button says how much of this job an earlier run built, while some is left.
function startLabel(done, of) {
  const label = document.querySelector('#start-button [data-localize]');
  if (!label) return;
  label.textContent = done && done < of ? t('plan_resume', 'Resume ({n} of {m})', { n: done, m: of }) : t('start_generation', 'Start Generation');
}

async function updatePlan() {
  const { req, flags, key } = planWanted();
  if (key === planKey) return;
  planKey = key;
  const seq = ++planSeq;
  if (!req) {
    if (!running) {
      lastPlan = null;
      onPlan();
    }
    showPlan([]);
    startLabel(0);
    if (!pieces.size) postToMap({ type: 'pieceOutlines', pieces: [] });
    return;
  }
  let plan;
  try {
    plan = await call('gui_plan_job', { ...req, flags });
  } catch (e) {
    if (seq !== planSeq) return;
    showPlan([div('', String(e))]);
    startLabel(0);
    return;
  }
  if (seq !== planSeq) return;
  const units = plan.units || [];
  const e = plan.estimate || {};
  const done = units.filter((u) => u.done).length;
  lastPlan = { n: units.length, w: e.workers };
  onPlan();
  planSizes.clear();
  units.forEach((u) => planSizes.set(u.index, u.size));
  startLabel(done, units.length);
  const regions = (list) => list.reduce((n, u) => n + u.regions, 0);
  const ring = regions(units.filter((u) => u.ring));
  const totals = { pieces: units.length, regions: regions(units), ring };
  // "30 pieces", a muted line of what else it is, then what it costs: disk, RAM, time.
  const stats = div('plan-stats', '');
  for (const [kind, key, tip, text] of [
    ['disk', 'plan_disk_tip', 'Disk space', range(e.disk_mb, gb)],
    ['ram', 'plan_ram_tip', 'Memory per worker', t('plan_ram_worker', '{ram}/worker', { ram: range(e.ram_mb, gb) })],
    ['time', 'plan_time_tip', 'Time (estimate, warm caches)', range(e.time_min, minutes)],
  ]) stats.append(Object.assign(div(`plan-stat plan-stat-${kind}`, text), { title: t(key, tip) }));
  showPlan([
    div('plan-big', t('plan_pieces', '{n} pieces', { n: totals.pieces })),
    div('plan-sub', [
      t('plan_regions', '{n} regions', { n: totals.regions }),
      ...(ring ? [t('plan_ring', 'ring {n}', { n: ring })] : []),
      t('plan_workers', '{n} workers', { n: e.workers }),
    ].join(' · ')),
    stats,
    ...(units.length > MAX_DRAWN ? [div('', t('plan_too_many', 'Too many pieces to draw on the map.'))] : []),
  ], e.free_disk_mb != null && e.disk_mb[1] > e.free_disk_mb ? t('plan_low_disk', 'Only {free} free on this disk.', { free: gb(e.free_disk_mb) }) : '',
  units.some((u) => u.ring));
  // A job's own outlines (running, or left to retry) stay until they go stale.
  if (pieces.size) return;
  const planned = units.length > MAX_DRAWN ? [] : units.map((u) => {
    const state = u.done ? 'done' : u.existing_chunks >= u.chunks ? 'skipped' : 'planned';
    const [key, fallback] = state === 'planned' ? ['plan_state', 'Planned'] : STATE_TEXT[state];
    return {
      i: u.index, n: u.index + 1, state, ring: u.ring, bounds: u.bbox.split(',').map(Number),
      text: { piece: t('run_piece', 'Piece {n}', { n: u.index + 1 }), state: t(key, fallback), size: sizeText(u.index), worker: '', error: '', hint: '' },
    };
  });
  postToMap({ type: 'pieceOutlines', pieces: planned });
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
  invoke('gui_local_archive_info', { tilesPath: '' })
    .then((info) => { defaultArchive = (info && info.default_folder) || ''; })
    .catch(() => {});

  el('meld-prepare-check').addEventListener('click', async () => {
    if (!area()) return say(list, needArea());
    say(list, t('meld_asking_geofabrik', 'Asking Geofabrik...'));
    try {
      const p = await invoke('gui_prepare_plan', { bboxText: area(), folder: folder(), tilesPath: '' });
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
        bboxText: area(), folder: folder(), tilesPath: '', flags: plan().flags,
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
