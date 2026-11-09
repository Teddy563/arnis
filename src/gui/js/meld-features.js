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

// piece index -> { state, bounds }, from the job's `piece` records.
const pieces = new Map();
let paused = false;
let retryNext = [];
let call = () => {};
let postToMap = () => {};
let restart = () => {};
let localizer = () => {};

const outlines = () =>
  [...pieces].map(([i, p]) => ({ i, state: p.state, bounds: p.bounds }));

export function initRunControls({ invoke, post, start, localize }) {
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
    retryNext = [...pieces].filter(([, p]) => RETRYABLE.has(p.state)).map(([i]) => i + 1);
    // Taken by the run's flags (meldFlags); one a refused start leaves behind is dropped here.
    Promise.resolve(restart()).finally(() => { retryNext = []; });
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
