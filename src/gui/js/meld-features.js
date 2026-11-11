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
export const t = (key, fallback, vars = {}) => {
  let s = (window.localization && window.localization[key]) || fallback;
  for (const [k, v] of Object.entries(vars)) s = s.split(`{${k}}`).join(v);
  return s;
};

// The same, singular when n is 1: the language's `<key>_one` string, or English's own while no
// language is loaded; a language with a plain string only keeps it.
export const tn = (key, fallback, one, n) =>
  n === 1 && (window.localization?.[`${key}_one`] || !window.localization?.[key]) ? t(`${key}_one`, one, { n }) : t(key, fallback, { n });

// A count in the language's plural category (one / few / other ...): `<key>_<category>`, else
// `<key>_other`, else the plain `<key>`, else English's. The category comes from the language
// the app is showing (`arnis-language`, as language-selector.js); `lang` overrides it.
export const tp = (key, fallback, n, lang) => {
  let cat = n === 1 ? 'one' : 'other';
  try {
    lang ||= globalThis.localStorage?.getItem('arnis-language') || globalThis.navigator?.language || 'en';
    cat = new Intl.PluralRules(lang).select(n);
  } catch {}
  const L = window.localization || {};
  const k = [`${key}_${cat}`, `${key}_other`, key].find((x) => L[x]);
  return t(k || key, fallback, { n });
};

// The bar's time left: "45 s", "18 min", "2 h 10 min" (units spaced and written the same in every
// language, as the bar's `run_eta` text is the only thing that differs).
export function etaDuration(sec) {
  sec = Math.max(0, Math.round(sec));
  if (sec < 60) return `${sec} s`;
  const m = Math.floor(sec / 60);
  if (m < 60) return sec % 60 ? `${m} min ${sec % 60} s` : `${m} min`;
  return m % 60 ? `${Math.floor(m / 60)} h ${m % 60} min` : `${Math.floor(m / 60)} h`;
}

// What the status line shows for a progress payload. An error that carries `detail` keeps the
// short `message` on the line and the full text as its tooltip and in the log; a Region Download
// fallback is a notice in the user's language rather than the raw log line.
// A fallback notice stays on the status line for 8 s: a following ordinary line then moves only
// the bar. The end of a run (Error!, Done!, Stopped, a retry's "The retried pieces are built") always shows.
export const holdsNotice = (shown, message, since, now) =>
  !shown.notice && since > 0 && now - since < 8000 && !/^(Error!|Done!|Stopped|The retried pieces are built)/.test(message);

// Which world a run writes to. A prewarm only fills the caches: it has no world, so the backend
// must never be handed the last built one (a new-world run deletes its world when it ends).
// A One World is resolved from the save folder, so a prewarm with One World on still sends it.
export const generationTarget = ({ prewarm, oneWorld, savePath, worldPath }) =>
  prewarm ? { selectedWorld: oneWorld ? savePath : '', isNewWorld: false } : { selectedWorld: oneWorld ? savePath : worldPath, isNewWorld: true };

// What a run is sent as `selectionShapes`: One World builds the rectangle around the shapes (the
// bbox the map reports), in pieces they travel as the plan mask; only another run clips to them.
export const generationShapes = ({ oneWorld, shapes }) => (oneWorld ? null : shapes);

export function progressText({ message = '', detail = '' }) {
  if (message.startsWith('region_download_fallback')) {
    return { text: t('region_download_fallback', 'Region Download is not available for this selection, so Arnis used the default source.'), title: '', log: message, notice: true };
  }
  // The backend counts "pieces"; the window says cells.
  const built = /^Building pieces\.\.\. (\d+)\/(\d+) done$/.exec(message);
  if (built) return { text: t('run_building', 'Building cells… {done}/{total}', { done: built[1], total: built[2] }), title: detail, log: detail || null, notice: false };
  return { text: message, title: detail, log: detail || null, notice: false };
}

// The docked 3D preview's width after its top-left grip moved `dx` px (left grows it): 200 px up to
// 60 % of the window's width, and of its height at 16:10.
export const dockWidth = (start, dx, vw, vh) => Math.round(Math.max(200, Math.min(start - dx, vw * 0.6, vh * 0.6 * 1.6)));

// The grip: dragging or the arrow keys (20 px) resize `dock`; its right and bottom stay where they are.
// A click on the grip is not a click on the card, so it never opens the full preview.
export function initDockGrip(dock, grip) {
  let from = null; // { x, w } while dragging
  const size = (w) => { dock.style.width = `${w}px`; };
  const stop = (e) => e.stopPropagation();
  grip.addEventListener('click', stop);
  grip.addEventListener('pointerdown', (e) => {
    e.stopPropagation();
    e.preventDefault();
    from = { x: e.clientX, w: dock.offsetWidth };
    grip.setPointerCapture?.(e.pointerId);
  });
  grip.addEventListener('pointermove', (e) => {
    if (from) size(dockWidth(from.w, e.clientX - from.x, window.innerWidth, window.innerHeight));
  });
  const end = (e) => { if (from) { from = null; grip.releasePointerCapture?.(e.pointerId); } };
  grip.addEventListener('pointerup', end);
  grip.addEventListener('pointercancel', end);
  grip.addEventListener('keydown', (e) => {
    const step = { ArrowLeft: 20, ArrowUp: 20, ArrowRight: -20, ArrowDown: -20 }[e.key];
    if (!step) return;
    e.preventDefault();
    e.stopPropagation();
    size(dockWidth(dock.offsetWidth, -step, window.innerWidth, window.innerHeight));
  });
}

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
  if (dock && el('preview3d-grip')) initDockGrip(dock, el('preview3d-grip'));
  if (dock && window.ResizeObserver) {
    new ResizeObserver(() => {
      if (!document.body.classList.contains('map-first') || !dock.offsetWidth) return;
      try { localStorage.setItem('arnis-preview-dock-width', String(dock.offsetWidth)); } catch (_) { /* not kept */ }
    }).observe(dock);
  }
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
    add('--plan-mask', maskPath);
  }
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

// The outline drawn or picked on the map, as the file `--plan-mask` reads ('' for none): a
// job in pieces then builds only the pieces it touches.
let maskPath = '';
let maskSeq = 0;
export const hasPlanMask = () => Boolean(maskPath);
export async function setPlanMask(geojson) {
  const seq = ++maskSeq;
  let path = '';
  let error = '';
  if (geojson) {
    try {
      path = await call('gui_write_plan_mask', { geojson: JSON.stringify(geojson) });
    } catch (e) {
      error = String(e);
      window.arnisLog?.('warn', `Outline not used: ${e}`);
    }
  }
  if (seq !== maskSeq) return '';
  maskPath = path || '';
  schedulePlan();
  return error;
}

// ---- A job built in pieces: outlines on the map, Pause, Stop and Retry ----

// piece index -> { state, bounds, halo, worker, pct, since, end, error }, from the job's
// `piece` records. `since` is when it began running, `end` when it stopped.
const pieces = new Map();
let paused = false;
let running = false;
// Whether the overlap border is drawn (the Show overlap button).
let halo = false;
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
        piece: t('run_piece', 'Cell {n}', { n: i + 1 }),
        state: t(key, fallback),
        size: sizeText(i),
        worker: p.worker ? t('run_worker', 'Worker {n}', { n: p.worker }) : '',
        // The first line is the failure; the child's output after it stays in the log.
        error: (p.error || '').split('\n')[0],
        hint: canRetry(p) ? t('run_retry_hint', 'Click to retry this cell') : '',
        openLog: p.state === 'failed' && p.log ? t('run_open_log', 'Open log') : '',
      },
      log: p.state === 'failed' ? p.log : undefined,
    };
  });

// A finished job's failed or stopped piece can be rebuilt alone.
const canRetry = (p) => !running && (p.state === 'failed' || p.state === 'stopped');

// Under the bar: "2 failed", only above 0 (the title and the bar say the rest).
export function failedLine() {
  const n = [...pieces.values()].filter((p) => p.state === 'failed').length;
  return n ? tp('run_failed', '{n} failed', n) : '';
}

// When this run's first piece record came, so after any dialog: the clock and the ETA count
// only what it built. 0 until then.
let runStart = 0;

// The bar's tooltip: "3:12 elapsed · 4 running" (the estimate is on the bar itself).
function barTip() {
  if (!running || !runStart) return '';
  const s = Math.floor((Date.now() - runStart) / 1000);
  const clock = `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  return t('run_tip', '{elapsed} elapsed · {running} running', { elapsed: clock, running: [...pieces.values()].filter((p) => RUNNING.has(p.state)).length });
}

function showRunLine() {
  el('run-failed').textContent = pieces.size ? failedLine() : '';
  const bar = el('progress-bar')?.parentElement;
  if (bar) {
    const tip = barTip();
    bar.title = tip;
    bar.tabIndex = tip ? 0 : -1; // focus shows it to a screen reader too
    if (tip) bar.setAttribute?.('aria-label', tip); else bar.removeAttribute?.('aria-label');
  }
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
  // The legend shows what the map was last sent.
  postToMap = (m) => {
    if (m.type === 'pieceOutlines') {
      drawn = m.pieces;
      if (legendOpen) showPlan(...shown); // an open card lists the lines now on the map
    }
    post(m);
  };
  restart = start;
  // The bar's tooltip ticks once a second while a job runs.
  setInterval(() => running && showRunLine(), 1000);
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
    if (!m || event.source !== mapWindow()) return;
    const p = pieces.get(m.piece);
    if (m.type === 'openLog') {
      if (p?.state === 'failed' && p.log) call('gui_show_in_folder', { path: p.log }).catch(() => {});
      return;
    }
    if (m.type === 'retryPiece' && p && canRetry(p)) retry([m.piece + 1]);
  });
  // The faint border around each piece showing what it builds beyond its own area.
  el('run-halo-toggle').addEventListener('click', (e) => {
    halo = e.currentTarget.getAttribute('aria-pressed') !== 'true';
    e.currentTarget.setAttribute('aria-pressed', String(halo));
    postToMap({ type: 'pieceHalo', on: halo });
    if (legendOpen) showPlan(...shown); // the card's overlap row follows the toggle
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
  if (!runStart) runStart = Date.now();
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
    log: piece.log || was?.log,
  });
  postToMap({ type: 'pieceOutlines', pieces: outlines() });
}

// A new run starts clean; a retry keeps the pieces it does not rebuild. `bbox` is the selection
// the run is for.
export function clearPieces() {
  runStart = 0;
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
  showRunLine();
  // A run that starts puts its own line in place of the plan; one that ended built pieces,
  // so the plan's states are read again.
  if (wasRunning !== running) showPlan(...shown); // the stretch line goes with the run
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
// The last plan drawn, { n pieces, w workers, done }, while it is the current one; null otherwise.
let lastPlan = null;
export const planPieces = () => lastPlan;
// The key of the request the plan last answered (or failed); a wanted plan with another key is
// being recomputed, and the size warning waits for it.
let planAnswered = '';
export const planPending = () => { const { key } = planWanted(); return key !== '' && key !== planAnswered; };
// Whether Start would run a job in pieces (it asks first: main.js, the hold to confirm).
export const isPiecesJob = () => Boolean(planWanted().req);
let onPlan = () => {};

export function initJobPlan({ request, changed = () => {}, stretch = () => null }) {
  planRequest = request;
  onPlan = changed;
  stretchSource = stretch;
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
  refreshStretch();
  if (fresh) planKey = planAnswered = '';
  if (planWanted().key !== planKey) {
    planSeq++;
    // A running job keeps its "Builds in n pieces" line.
    if ((lastPlan || planPending()) && !running) {
      lastPlan = null;
      onPlan();
    }
    if (!pieces.size) postToMap({ type: 'pieceOutlines', pieces: [] });
    // A finished job's pieces (and Retry) belong to its own plan, not to this one.
    if (!running && pieces.size && keyOf() !== jobKey) resetPieces();
  }
  clearTimeout(planTimer);
  planTimer = setTimeout(updatePlan, 250);
}

// The benchmark winner (Nov 2026): cells of 8 regions, 4 workers, from this area up. Below it
// Auto builds in one plain run, so Auto stays.
export const RECOMMENDED_FROM_KM2 = 220;
export function useRecommended(areaKm2) {
  const big = areaKm2 >= RECOMMENDED_FROM_KM2;
  const workers = el('meld-workers-select');
  workers.value = big ? '4' : 'auto';
  workers.dispatchEvent(new Event('change', { bubbles: true }));
  document.querySelector(`#meld-piece-size-group .segment[data-v="${big ? '8' : 'auto'}"]`)?.click();
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

// A long job in whole hours ("48–139 h"); a shorter one as `minutes`.
const longTime = (hours) => (m) => (hours ? t('plan_hours_only', '{h} h', { h: Math.max(1, Math.round(m / 60)) }) : minutes(m));

// The plan's headline (time and size, one small row, then the disk line), and a "?" with everything else.
// `more` is the lines behind the "?". `disk` is null while the free space is unknown.
export function planHead(e, more) {
  const head = div('plan-head', '');
  // One small row, one value each: the midpoint of the estimate, never a range (the "?" keeps the ranges).
  const mid = ([lo, hi]) => (lo + hi) / 2;
  const ram = mid(e.ram_mb) * (e.workers || 1);
  for (const [kind, key, tip, text] of [
    ['time', 'plan_time_tip', 'Time (estimate, warm caches)', `≈ ${longTime(e.time_min[1] >= 600)(mid(e.time_min))}`],
    ['ram', 'plan_ram_tip', 'Memory per worker', `≈ ${gb(ram)} RAM`],
    ['disk', 'plan_disk_tip', 'Disk space', `≈ ${gb(mid(e.disk_mb))}`],
  ]) head.append(Object.assign(div(`plan-stat plan-stat-${kind}`, text), { title: t(key, tip) }));
  const tip = Object.assign(div('plan-tip', ''), { id: 'plan-tip', role: 'tooltip' });
  tip.append(...[`${range(e.time_min, longTime(e.time_min[1] >= 600))} · ${range(e.disk_mb, gb)}`, ...more].map((x) => div('', x)));
  // The one "?": hover or focus shows the details; a click holds them open, with the legend card (showPlan).
  const help = Object.assign(document.createElement('button'), { type: 'button', className: 'tooltip-icon plan-help', id: 'legend-help', textContent: '?', title: t('plan_more', 'Details') });
  help.setAttribute('aria-describedby', 'plan-tip');
  const toggle = (open) => {
    legendOpen = open;
    showPlan(...shown);
    el('legend-help')?.focus();
  };
  help.addEventListener('click', () => toggle(!legendOpen));
  help.addEventListener('keydown', (ev) => ev.key === 'Escape' && legendOpen && toggle(false));
  head.append(help, tip);
  head.tip = tip;
  head.help = help;
  const fits = e.free_disk_mb == null ? null : e.disk_mb[1] <= e.free_disk_mb;
  const disk = div(`plan-disk${fits === false ? ' is-low' : ''}`, fits == null ? '' : fits
    ? t('plan_disk_fits', 'Fits: {free} free', { free: gb(e.free_disk_mb) })
    : t('plan_disk_short', 'Needs {need}, {free} free on this disk', { need: range(e.disk_mb, gb), free: gb(e.free_disk_mb) }));
  disk.hidden = fits == null;
  return [head, disk];
}

// "4 × 4 regions" for the piece numbered `i` in the last plan.
const planSizes = new Map();
const sizeText = (i) => (planSizes.has(i) ? t('plan_piece_size', '{n} × {m} regions', { n: planSizes.get(i), m: planSizes.get(i) }) : '');

const div = (className, textContent) => Object.assign(document.createElement('div'), { className, textContent });

// The plan's blocks, a warning, and the legend of what the map shows: the selection, the
// pieces to build and, with a margin, the ring around it.
function showPlan(rows, warn = '', ring = false, single = false) {
  shown = [rows, warn, ring, single];
  const box = el('job-plan');
  if (!box) return;
  // With a plan the stretch lines sit behind its "?" (amber or red there too); alone they show as they are.
  const head = rows[0]?.tip ? rows[0] : null;
  const stretch = running ? [] : stretchRows; // the stretch readout is for planning, not for a run
  if (head) {
    head.tip.querySelectorAll?.('.plan-stretch, .legend-card').forEach((x) => x.remove());
    head.tip.append(...stretch);
    head.help.classList.toggle('is-open', legendOpen);
    head.help.ariaExpanded = String(legendOpen);
    // The "?" opens the card of every line the map can show right now, inside the details.
    if (legendOpen && rows.length > 1) head.tip.append(legendCard());
    head.help.classList.toggle('is-warn', stretch.some((x) => x.classList.contains('is-warn')));
    head.help.classList.toggle('is-error', stretch.some((x) => x.classList.contains('is-error')));
  }
  box.replaceChildren(...rows, ...(head ? [] : stretch));
  if (warn) box.append(div('job-plan-warn', warn));
  if (rows.length > 1) {
    const legend = div('job-plan-legend', '');
    const keys = [['selection', 'plan_legend_selection', 'Selection'], ...(single ? [] : [['pieces', 'plan_legend_pieces', 'Cells to build']])];
    if (ring) keys.push(['ring', 'plan_legend_ring', 'Ring (margin)']);
    for (const [kind, key, fallback] of keys) {
      const item = document.createElement('span');
      item.append(Object.assign(document.createElement('i'), { className: `swatch swatch-${kind}` }), t(key, fallback));
      legend.append(item);
    }
    box.append(legend);
  }
  box.hidden = !rows.length && !stretch.length;
}

// ---- What the lines on the map mean ----

// Every row is drawn from the map's own tables (piece-style.js), only when it is on the map.
// Words: the plan's and the run's own keys where they say the same, a few of its own.
const LEGEND_TEXT = {
  legend_title: 'What the lines on the map mean',
  legend_drawing: 'Your selection while you draw it',
  plan_legend_selection: 'Selection',
  legend_shape_rect: 'The rectangle One World builds around your shapes',
  legend_shape: 'The shape you drew; a cell it touches is built in full',
  plan_state: 'Planned',
  run_state_queued: 'Waiting',
  run_state_running: 'Running',
  run_state_done: 'Done',
  run_state_failed: 'Failed',
  run_state_stopped: 'Stopped',
  plan_state_excluded: 'Outside the outline: not built',
  plan_state_room: 'Room to grow: you can build here later',
  plan_legend_ring: 'Ring (margin)',
  legend_overlap: 'Overlap two neighbouring cells share',
  legend_stretch_5: 'Past this line the map is stretched 5% against real distances',
  legend_stretch_10: 'Past this line the stretch is 10%; start a new world instead',
  legend_held: 'Land this world already holds',
};

// state: { drawing, selection, shape, cells: [{ state, ring }], halo, stretch: [0.05, 0.1], held }
// -> [{ swatch: { color, weight, fill, dash?, line? }, key }]
export function legendRows(state) {
  const { colors, dash, selection, selectionDash, mask } = globalThis.arnisPieceStyle;
  const rows = [];
  const add = (show, key, color, swatch = {}) => show && rows.push({ key, swatch: { color, weight: 2, fill: 0, ...swatch } });
  const has = (...states) => state.cells.some((c) => states.includes(c.state));
  add(state.drawing, 'legend_drawing', selection, { weight: 3, fill: 0.1, dash: selectionDash });
  add(state.selection, 'plan_legend_selection', selection, { weight: 3, fill: 0.08 });
  add(state.shape && !state.rect, 'legend_shape', mask, { fill: 0.05 });
  add(state.rect, 'legend_shape_rect', selection, { dash: selectionDash });
  add(state.cells.some((c) => c.state === 'planned' && !c.ring), 'plan_state', colors.planned, { weight: 1, fill: 0.08 });
  add(has('queued'), 'run_state_queued', colors.queued, { dash: dash.queued });
  add(has('start', 'retry'), 'run_state_running', colors.start, { dash: dash.start });
  add(has('done', 'skipped'), 'run_state_done', colors.done);
  add(has('failed'), 'run_state_failed', colors.failed, { weight: 3 });
  add(has('stopped'), 'run_state_stopped', colors.stopped, { dash: dash.stopped });
  add(has('excluded'), 'plan_state_excluded', colors.excluded, { weight: 1, fill: 0.3 });
  add(has('room'), 'plan_state_room', colors.room, { weight: 1.5, dash: dash.room });
  add(state.cells.some((c) => c.state === 'planned' && c.ring), 'plan_legend_ring', colors.planned, { weight: 1, fill: 0.03, dash: dash.ring });
  add(state.halo, 'legend_overlap', colors.queued, { weight: 1, dash: dash.halo });
  for (const [level, color] of STRETCH_LEVELS) {
    add(state.stretch.includes(level), `legend_stretch_${Math.round(level * 100)}`, color, { line: true, dash: dash.stretch });
  }
  add(state.held, 'legend_held', selection, { weight: 1 });
  return rows;
}

// The last outlines sent to the map, and whether the legend card is open.
let drawn = [];
let legendOpen = false;

function legendCard() {
  const map = mapWindow()?.arnisLegendState?.() || {};
  const rows = legendRows({
    drawing: map.drawing, selection: map.selection, held: map.held, rect: map.rect, shape: hasPlanMask(),
    cells: drawn, halo, stretch: stretchShown,
  });
  const card = div('legend-card', '');
  Object.assign(card, { role: 'group', ariaLabel: t('legend_title', LEGEND_TEXT.legend_title) });
  const svg = (name, attrs) => Object.entries(attrs).reduce((n, [k, v]) => (n.setAttribute(k, v), n), document.createElementNS('http://www.w3.org/2000/svg', name));
  for (const { swatch: w, key } of rows) {
    const line = { stroke: w.color, 'stroke-width': w.weight, 'stroke-dasharray': w.dash || 'none' };
    const mark = svg('svg', { viewBox: '0 0 24 12', width: 24, height: 12, 'aria-hidden': 'true' });
    mark.append(w.line
      ? svg('line', { x1: 0, x2: 24, y1: 6, y2: 6, ...line })
      : svg('rect', { x: 2, y: 2, width: 20, height: 8, fill: w.color, 'fill-opacity': w.fill, ...line }));
    const row = document.createElement('div');
    const swatch = Object.assign(document.createElement('i'), { className: 'swatch' });
    swatch.append(mark);
    row.append(swatch, t(key, LEGEND_TEXT[key]));
    card.append(row);
  }
  return card;
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
    if (!pieces.size) postToMap({ type: 'pieceOutlines', pieces: [] });
    return;
  }
  let plan;
  try {
    plan = await call('gui_plan_job', { ...req, flags });
  } catch (e) {
    if (seq !== planSeq) return;
    planAnswered = key;
    showPlan([div('', t('plan_error', 'Could not plan this selection: {error}', { error: e }))]);
    onPlan();
    return;
  }
  if (seq !== planSeq) return;
  const units = plan.units || [];
  const e = plan.estimate || {};
  const done = units.filter((u) => u.done).length;
  // `done`: how much of this job an earlier run built (main.js says "Resume (n of m)").
  // `overlap`: pieces on land the world already has, which the run builds again (Skip Built
  // leaves those out, so then it is 0).
  const overlap = on('meld-skip-built-toggle') ? 0 : units.filter((u) => u.existing_chunks > 0).length;
  planAnswered = key;
  lastPlan = { n: units.length, w: e.workers, done, overlap, single: plan.single === true };
  onPlan();
  planSizes.clear();
  units.forEach((u) => planSizes.set(u.index, u.size));
  const regions = (list) => list.reduce((n, u) => n + u.regions, 0);
  const ring = regions(units.filter((u) => u.ring));
  const totals = { pieces: units.length, regions: regions(units), ring };
  // Time and size large, the disk line, the rest behind the "?".
  // A small area is built in one run: no cells to count or draw.
  const single = plan.single === true;
  showPlan(planHead(e, [
    single ? t('plan_single', 'Small area: built in one run.') : [
      tn('plan_pieces', '{n} cells', '{n} cell', totals.pieces),
      tn('plan_regions', '{n} regions', '{n} region', totals.regions),
      ...(ring ? [t('plan_ring', 'ring {n}', { n: ring })] : []),
      tn('plan_workers', '{n} workers', '{n} worker', e.workers),
    ].join(' · '),
    `${t('plan_ram_tip', 'Memory per worker')}: ${range(e.ram_mb, gb)}`,
  ]), '', units.some((u) => u.ring), single);
  if (single && !pieces.size) {
    postToMap({ type: 'pieceOutlines', pieces: [] });
    return;
  }
  // A job's own outlines (running, or left to retry) stay until they go stale.
  if (pieces.size) return;
  const planned = units.map((u) => {
    const state = u.done ? 'done' : on('meld-skip-built-toggle') && u.existing_chunks >= u.chunks ? 'skipped' : 'planned';
    const [key, fallback] = state === 'planned' ? ['plan_state', 'Planned'] : STATE_TEXT[state];
    return {
      i: u.index, n: u.index + 1, state, ring: u.ring, bounds: u.bbox.split(',').map(Number),
      text: { piece: t('run_piece', 'Cell {n}', { n: u.index + 1 }), state: t(key, fallback), size: sizeText(u.index), worker: '', error: '', hint: '' },
    };
  });
  // Outside the outline: grey, unnumbered, not built (numbered after the plan's own, as ids).
  // Around the job, within 5% stretch: faint, dotted, room to grow later.
  const extra = [
    ...(plan.excluded || []).map((bbox) => ['excluded', bbox, t('plan_state_excluded', 'Outside the outline: not built')]),
    ...(plan.room || []).map((bbox) => ['room', bbox, t('plan_state_room', 'Room to grow: you can build here later')]),
  ];
  if (planned.length) {
    extra.forEach(([state, bbox, text], k) => planned.push({
      i: units.length + k, n: '', state, ring: false, bounds: bbox.split(',').map(Number),
      text: { piece: '', state: text, size: '', worker: '', error: '', hint: '' },
    }));
  }
  postToMap({ type: 'pieceOutlines', pieces: planned });
}

// ---- How far a One World can grow before it looks stretched ----

// Blocks per metre are fixed at the origin's latitude and heights are not, so at latitude `lat`
// the ground is this much wider (+) or narrower (-) than it is tall. Only latitude counts.
const rad = Math.PI / 180;
export const stretchAt = (origin, lat) => Math.cos(origin * rad) / Math.cos(lat * rad) - 1;

// The nearest latitudes south and north of `origin` where the stretch reaches `s` either way
// (cos lat = cos origin / (1 ± s)); null on a side that has none.
export function stretchLines(origin, s) {
  const lats = [];
  for (const k of [1 + s, 1 - s]) {
    const c = Math.cos(origin * rad) / k;
    if (c <= 1) lats.push(Math.acos(c) / rad, -Math.acos(c) / rad);
  }
  const south = lats.filter((l) => l < origin);
  const north = lats.filter((l) => l > origin);
  return [south.length ? Math.max(...south) : null, north.length ? Math.min(...north) : null];
}

// The stretch furthest from none over [south, north] (it grows away from the equator).
export function worstStretch(origin, south, north) {
  const lats = south < 0 && north > 0 ? [south, north, 0] : [south, north];
  return lats.map((l) => stretchAt(origin, l)).reduce((a, b) => (Math.abs(b) > Math.abs(a) ? b : a));
}

// `stretchSource()`: { origin, knee, south, north, lon } for a One World selection, or null.
let stretchSource = () => null;
let stretchRows = [];
let stretchKey = 'null';
let shown = [[], '', false];
// The stretch levels drawn on the map now (STRETCH_LEVELS), for the legend.
let stretchShown = [];
const STRETCH_LEVELS = [[0.05, '#fecc44'], [0.1, '#fa7878']];

export function refreshStretch() {
  // Off by default: no row in the plan, no lines or labels on the map, no legend rows.
  const s = on('advanced-meld-features-toggle') && on('meld-stretch-toggle') ? stretchSource() : null;
  const key = JSON.stringify(s);
  if (key === stretchKey) return;
  stretchKey = key;
  stretchRows = [];
  const lines = [];
  if (s) {
    for (const [level, color] of STRETCH_LEVELS) {
      for (const lat of stretchLines(s.origin, level)) {
        if (lat !== null) lines.push({ lat, color, label: `${level * 100}%`, lon: s.lon });
      }
    }
    const worst = worstStretch(s.origin, s.south, s.north);
    const pct = Math.round(worst * 100);
    const row = div('plan-stretch', t('plan_stretch', 'Stretch here: {pct} · up to 5% recommended', {
      pct: `${pct > 0 ? '+' : pct < 0 ? '−' : ''}${Math.abs(pct)}%`,
    }));
    row.classList.toggle('is-warn', Math.abs(worst) > 0.05);
    row.classList.toggle('is-error', Math.abs(worst) > 0.1);
    stretchRows.push(row);
    // The selection's heights are not known here: said as information.
    if (s.knee != null) {
      stretchRows.push(div('plan-stretch', t('plan_soft_top', 'Land above {m} m is squashed', {
        m: (Math.round(s.knee / 100) * 100).toLocaleString(),
      })));
    }
  }
  stretchShown = STRETCH_LEVELS.filter(([, color]) => lines.some((l) => l.color === color)).map(([level]) => level);
  postToMap({ type: 'stretchLines', lines });
  showPlan(...shown);
}

// ---- Data panel: Prepare Countries and the Download Plan ----

// Sizes for the Download Plan; nothing missing prints nothing, anything missing at least 1 MB.
export const mb = (b) => (b <= 0 ? '0 MB' : b >= 1e12 ? `${(b / 1e12).toFixed(1)} TB` : b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : `${Math.max(1, Math.round(b / 1e6))} MB`);

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

const SOURCE_NAMES = { osm: 'Map data', elevation: 'Elevation', land_cover: 'Land cover', overture: 'Building footprints' };

// `area()` is the selected bbox; `plan()` the settings a run's plan depends on.
export function initDataPanel({ invoke, area, plan, prefetch }) {
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
      say(list, t('plan_error', 'Could not plan this selection: {error}', { error: e }));
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
      say(list, t('plan_error', 'Could not plan this selection: {error}', { error: e }));
    }
    stop.hidden = true;
  });
  stop.addEventListener('click', () => invoke('gui_cancel_bake'));

  const where = el('meld-plan-where');
  let cacheRoot = '';
  const showPath = () => {
    el('meld-plan-path').textContent = cacheRoot ? t('meld_cache_path', 'Files are kept in {path}', { path: cacheRoot }) : '';
    where.hidden = !cacheRoot;
  };
  // The path line is built from a stored path, so a language switch only redraws it.
  window.addEventListener('arnis-localized', showPath);
  const check = async (ready = false) => {
    if (!area()) return say(out, needArea());
    try {
      const p = await invoke('gui_data_plan', { bboxText: area(), ...plan() });
      const rows = p.items.map((i) => {
        const name = t(`data_source_${i.source}`, SOURCE_NAMES[i.source] || i.source);
        if (!i.total) return [name, t('meld_size_unknown', 'size unknown')];
        return [t('meld_cached', '{source}: {cached} of {total} cached', { source: name, cached: i.cached, total: i.total }), i.missing_bytes != null ? (i.missing_bytes > 0 ? t('meld_to_download', '{size} to download', { size: mb(i.missing_bytes) }) : '') : t('meld_size_unknown', 'size unknown')];
      });
      if (p.free_bytes != null) rows.push([t('meld_free_space', 'Free space'), mb(p.free_bytes)]);
      if (ready) rows.unshift([t('meld_ready_offline', 'Ready to build offline.'), '']);
      lines(out, rows.length ? rows : [[t('meld_nothing_to_download', 'Nothing to download.'), '']]);
      cacheRoot = p.cache_root || '';
      showPath();
    } catch (e) {
      say(out, t('plan_error', 'Could not plan this selection: {error}', { error: e }));
    }
  };
  el('meld-plan-show').addEventListener('click', () => check());
  el('meld-plan-open').addEventListener('click', () => cacheRoot && invoke('gui_show_in_folder', { path: cacheRoot }).catch(() => {}));

  // Download now: a run with --prewarm fills the caches and writes no world. The run's own
  // progress lines come through the window's progress-update event.
  let prefetching = false;
  const listen = window.__TAURI__?.event?.listen;
  if (listen) {
    listen('progress-update', (e) => {
      const m = e.payload && e.payload.message;
      if (!prefetching || !m || e.payload.piece) return;
      // Same text as the main status line (a fallback notice is translated).
      if (m.startsWith('Done!')) {
        prefetching = false;
        check(true);
      } else {
        if (/^(Error!|Stopped)/.test(m)) prefetching = false;
        say(out, progressText(e.payload).text);
      }
    });
  }
  el('meld-plan-download').addEventListener('click', async () => {
    if (!area()) return say(out, needArea());
    // Set before the run starts so its first lines are caught; cleared when Start refused.
    prefetching = true;
    say(out, t('meld_downloading', 'Downloading. Progress shows on the main bar.'));
    if (!(await prefetch())) {
      prefetching = false;
      check();
    }
  });
}

// Settings > Projects: the One Worlds in the save folder, as a list or a grid of cards.
// `open(name, expand)` makes one the target; the name is stored here so the choice outlives the
// session. Every field the engine adds later (place name, thumbnail, jobs) may be missing.
// Returns a refresh function.
const VIEW_KEY = 'arnis-projects-view';
const readView = () => {
  try { return localStorage.getItem(VIEW_KEY) === 'grid' ? 'grid' : 'list'; } catch (_) { return 'list'; }
};

export function initProjects({ invoke, savePath, scale, open, available = () => true }) {
  const list = el('projects-list');
  let view = readView();
  const ratio = (s) => (s >= 1 ? `${+s.toFixed(2)}:1` : `1:${+(1 / s).toFixed(2)}`);
  const button = (key, fallback, disabled, onClick) => {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'settings-action-button';
    b.disabled = disabled;
    b.textContent = t(key, fallback);
    b.addEventListener('click', onClick);
    return b;
  };
  const text = (cls, s) => {
    const span = document.createElement('span');
    span.className = cls;
    span.textContent = s;
    return span;
  };
  const pick = async (name, expand) => {
    localStorage.setItem('arnis-one-world-name', name);
    localStorage.setItem('arnis-one-world-picked', '1');
    await open(name, expand);
  };

  // A thumbnail is asked for when its card scrolls into view; no image leaves the placeholder.
  const thumbs = new Map();
  const loaders = new Map();
  const seen = typeof IntersectionObserver === 'undefined' ? null : new IntersectionObserver((entries) => {
    for (const e of entries) {
      if (!e.isIntersecting) continue;
      seen.unobserve(e.target);
      loaders.get(e.target)?.();
    }
  }, { rootMargin: '200px' });
  const thumb = (w) => {
    const box = document.createElement('div');
    box.className = 'projects-thumb is-empty';
    const path = w.world_path || `${savePath()}/${w.name}`;
    const key = `${path}|${w.modified_ms || 0}`;
    const show = (url) => {
      if (!url) return;
      const img = document.createElement('img');
      img.alt = '';
      img.src = url;
      box.className = 'projects-thumb';
      box.replaceChildren(img);
    };
    const load = async () => {
      if (!thumbs.has(key)) {
        let url = null;
        try { url = w.preview === false ? null : await invoke('gui_world_thumbnail', { worldPath: path }); } catch (_) { /* a world without an image */ }
        thumbs.set(key, url || null);
      }
      show(thumbs.get(key));
    };
    if (thumbs.has(key)) show(thumbs.get(key));
    else if (seen) { loaders.set(box, load); seen.observe(box); } else load();
    return box;
  };

  const setView = (v) => {
    view = v;
    try { localStorage.setItem(VIEW_KEY, v); } catch (_) { /* not kept */ }
    paint();
    refresh();
  };
  const paint = () => {
    list.className = view === 'grid' ? 'projects-list is-grid' : 'projects-list';
    for (const v of ['list', 'grid']) el(`projects-view-${v}`)?.setAttribute('aria-pressed', String(v === view));
  };
  el('projects-view-list')?.addEventListener('click', () => setView('list'));
  el('projects-view-grid')?.addEventListener('click', () => setView('grid'));
  paint();

  async function refresh() {
    let worlds = [];
    try { worlds = savePath() ? (await invoke('gui_list_one_worlds', { savePath: savePath() })) || [] : []; } catch (e) { console.error(e); }
    const rows = worlds.map((w) => {
      const row = document.createElement('div');
      row.className = 'projects-row';
      const odd = w.scale != null && scale() != null && Math.abs(w.scale - scale()) > 1e-6;
      const title = document.createElement('div');
      title.className = 'projects-title';
      title.append(text('projects-name', w.name));
      if (w.place_name) title.append(text('projects-place', w.place_name));
      const kids = [
        thumb(w),
        title,
        text('projects-meta', t('projects_areas', 'Areas: {n}', { n: w.area_count })),
        text(odd ? 'projects-meta is-warn' : 'projects-meta', t('projects_scale', 'Scale {scale}', { scale: w.scale ? ratio(w.scale) : '-' })),
      ];
      if (w.modified_ms) kids.push(text('projects-meta', new Date(w.modified_ms).toLocaleDateString()));
      if (w.locked) kids.push(text('projects-meta is-warn', t('projects_in_use', 'In use')));
      // One World needs Java on Earth: without it the buttons would do nothing.
      const off = !available();
      kids.push(button('projects_open', 'Open', w.locked || off, () => pick(w.name, false)));
      kids.push(button('projects_expand', 'Expand', w.locked || off, () => pick(w.name, true)));
      if (off) kids.slice(-2).forEach((b) => { b.title = t('one_world_desc_on', 'Each area joins one growing world at its real position. Java only.'); });
      row.append(...kids);
      return row;
    });
    if (!rows.length) rows.push(text('projects-empty', t('projects_empty', 'No worlds yet. Build one with One World on.')));
    list.replaceChildren(...rows);
  }
  return refresh;
}
