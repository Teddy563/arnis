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
export function meldFlags({ oneWorld }) {
  if (!on('advanced-meld-features-toggle')) return [];
  const f = [];
  const add = (flag, v) => v !== '' && f.push(`${flag}=${v}`);

  // Scale
  if (oneWorld) add('--one-world-workers', value('meld-workers-select'));
  const threads = value('meld-threads-input');
  if (threads) add('--threads', threads);
  else if (value('meld-cpu-target-input') !== '90') add('--cpu-target', value('meld-cpu-target-input'));
  add('--ram-budget-mb', value('meld-ram-budget-input'));

  return f;
}
