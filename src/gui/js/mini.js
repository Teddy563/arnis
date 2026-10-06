// Mini mode (Settings > Application). With it on, closing the window, or the
// shrink button by the progress bar, turns the window into a small panel with
// the run's stats instead of quitting: gui.rs gui_mini_window resizes the same
// window and the CloseRequested hook sends "mini-close-requested". It stays
// the same page, so a run keeps going and its progress events keep arriving.
// Off, the hook does nothing and closing quits as it always has.
const invoke = (cmd, args) => window.__TAURI__?.core?.invoke?.(cmd, args);
const $ = (id) => document.getElementById(id);
const t = (key, fallback) => (window.localization && window.localization[key]) || fallback;
let mini = false;
let ticker = null;

// Attached at load, before main.js restores the settings, so a stored "on"
// reaches the backend through this handler.
$('mini-mode-toggle').addEventListener('change', (e) => {
  invoke('gui_set_mini_mode', { enabled: e.target.checked });
  $('mini-shrink').hidden = !e.target.checked;
});

async function enterMini() {
  if (mini) return;
  mini = true;
  document.body.classList.add('mini-mode');
  render();
  ticker = setInterval(render, 1000);
  try {
    await invoke('gui_mini_window', { mini: true });
  } catch (error) {
    console.error('Mini mode failed:', error);
    exitMini();
  }
}

async function exitMini() {
  if (!mini) return;
  mini = false;
  clearInterval(ticker);
  showConfirm(false);
  try {
    await invoke('gui_mini_window', { mini: false });
  } catch (error) {
    console.error('Leaving mini mode failed:', error);
  }
  document.body.classList.remove('mini-mode');
}

// Idle there is nothing to lose, so the X quits; a run asks first.
function requestClose() {
  if (window.arnisRunRunning?.()) showConfirm(true);
  else invoke('gui_quit');
}

function showConfirm(on) {
  $('mini-confirm').hidden = !on;
  if (on) $('mini-confirm-cancel').focus();
}

// Mirrors the main window, which keeps updating underneath.
function render() {
  const info = $('progress-info');
  const name = $('world-name-label');
  $('mini-world').textContent = name.dataset.placeholder === 'true' ? '' : name.textContent;
  $('mini-stage').textContent = info.textContent || t('mini_idle', 'Ready');
  $('mini-stage').style.color = info.textContent ? info.style.color : '';
  $('mini-bar').style.width = $('progress-bar').style.width;
  $('mini-percent').textContent = $('progress-detail').textContent;
  window.arnisFillRunStats?.($('mini-stats'));
  labels();
}

// Icon buttons, so their names are tooltips, set late enough to be localized.
function labels() {
  for (const [id, key, fallback] of [
    ['mini-shrink', 'mini_shrink', 'Shrink to mini panel'],
    ['mini-expand', 'mini_expand', 'Expand'],
    ['mini-close', 'mini_close', 'Close Arnis'],
  ]) {
    $(id).title = t(key, fallback);
    $(id).setAttribute('aria-label', $(id).title);
  }
}

$('mini-shrink').addEventListener('click', enterMini);
$('mini-shrink').addEventListener('pointerenter', labels);
$('mini-shrink').addEventListener('focus', labels);
$('mini-expand').addEventListener('click', exitMini);
$('mini-close').addEventListener('click', requestClose);
$('mini-confirm-cancel').addEventListener('click', () => showConfirm(false));
$('mini-confirm-stop').addEventListener('click', () => invoke('gui_quit'));
$('mini-confirm').addEventListener('keydown', (e) => {
  if (e.key === 'Escape') showConfirm(false);
});
// No title bar: the header moves the window.
$('mini-head').addEventListener('mousedown', (e) => {
  if (e.button === 0 && !e.target.closest('button')) invoke('gui_mini_drag');
});
window.__TAURI__?.event?.listen?.('mini-close-requested', () => (mini ? requestClose() : enterMini()));
// After main.js's own listener has moved the bar.
window.__TAURI__?.event?.listen?.('progress-update', () => mini && setTimeout(render));
