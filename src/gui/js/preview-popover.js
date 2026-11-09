// The picture that appears when the info icon of a setting, or one of its choice buttons, is
// hovered or focused: an isometric block diorama of what the setting does (src/gui/images/previews3d,
// made by scripts/previews3d), large and centred. Only settings that carry data-preview-on/-off (a
// switch), data-preview-seg (a segmented control, one picture per button value) or data-preview-sel (a
// select, one per option) get one.

const BASE = 'images/previews3d/';
const W = 720, H = 540; // the pictures' pixel size; the page shows them as large as the window allows (settings.css)
const MARGIN = 16; // px between the popover and the viewport edge

// File name (no extension) for a setting in its current state, or for `button`, a choice button the
// pointer is on. `el` is the setting's ? icon or its row and carries data-preview-on/-off (a switch),
// data-preview-num + data-preview-steps (a slider or number box, nearest step), data-preview-seg (+ optional data-preview-key: the dataset key that holds a button's value, default
// "v") or data-preview-sel (a <select>). Null when the setting has no picture for that state.
export function previewName(el, button) {
  const row = el.closest('.settings-row');
  if (!row) return null;
  if (el.dataset.previewSeg) {
    const b = button || row.querySelector('.segment.active');
    const v = b && b.dataset.nopreview === undefined && b.dataset[el.dataset.previewKey || 'v'];
    return v ? `${el.dataset.previewSeg}-${v}` : null;
  }
  if (el.dataset.previewSel) {
    const sel = row.querySelector('select');
    return sel && sel.value ? `${el.dataset.previewSel}-${sel.value}` : null;
  }
  if (el.dataset.previewNum) {
    // A slider or number box: the picture of the nearest of data-preview-steps ("0.5,1,2"), by size.
    const input = row.querySelector('input[type=range], input[type=number]');
    const v = input && Math.abs(parseFloat(input.value));
    if (!(v >= 0)) return null;
    const steps = el.dataset.previewSteps.split(',');
    const best = steps.reduce((a, c) => (Math.abs(c - v) < Math.abs(a - v) ? c : a));
    return `${el.dataset.previewNum}-${best}`;
  }
  const sw = row.querySelector('input.switch');
  return (sw && sw.checked ? el.dataset.previewOn : el.dataset.previewOff) || null;
}

export const captionKey = (name) => `preview_${name.replace(/[^a-z0-9]+/gi, '_')}`;

// Top-left corner for a popover of size `pop`: the centre of the window, kept inside it.
export function place(pop, vw, vh) {
  return {
    left: Math.max(MARGIN, (vw - pop.w) / 2),
    top: Math.max(MARGIN, (vh - pop.h) / 2),
  };
}

let started = false;

export function initPreviews() {
  if (started) return; // the popover is created once, whoever calls this
  started = true;
  const pop = document.createElement('div');
  pop.className = 'preview-popover';
  pop.id = 'preview-popover';
  pop.setAttribute('role', 'tooltip');
  pop.setAttribute('aria-hidden', 'true');
  const img = document.createElement('img');
  img.decoding = 'async';
  img.width = W;
  img.height = H;
  const caption = document.createElement('div');
  caption.className = 'preview-popover-caption';
  pop.appendChild(img);
  pop.appendChild(caption);
  document.body.appendChild(pop);

  let open = false;
  let current = null; // the icon the picture is for
  let want = null; // the file being loaded
  const hide = () => {
    open = false;
    want = null;
    if (current) current.removeAttribute('aria-describedby');
    pop.classList.remove('is-visible');
    pop.setAttribute('aria-hidden', 'true');
  };
  const show = (icon, button) => {
    const name = previewName(icon, button);
    if (!name) return hide();
    const text = (window.localization && window.localization[captionKey(name)]) || '';
    if (current && current !== icon) current.removeAttribute('aria-describedby');
    current = icon;
    icon.setAttribute('aria-describedby', pop.id);
    // The popover appears once the picture is in, so the one before it never flashes; the caption is the
    // text (the picture's alt is empty so a screen reader does not say it twice).
    pop.classList.remove('is-visible');
    img.onerror = hide; // a picture that is missing leaves the icon's text tooltip alone
    want = name;
    img.onload = () => {
      if (want === name) {
        pop.classList.add('is-visible');
        pop.setAttribute('aria-hidden', 'false');
      }
    };
    img.removeAttribute('src');
    img.alt = '';
    img.src = `${BASE}${name}.webp`;
    caption.textContent = text;
    const r = pop.getBoundingClientRect();
    const at = place({ w: r.width || W, h: r.height || H }, window.innerWidth, window.innerHeight);
    pop.style.left = `${at.left}px`;
    pop.style.top = `${at.top}px`;
    open = true;
  };

  // Every control of the setting shows it: the ? icon, the switch, the select, each choice button.
  const on = (n, f) => {
    for (const ev of ['mouseenter', 'focus']) n.addEventListener(ev, f);
    for (const ev of ['mouseleave', 'blur']) n.addEventListener(ev, hide);
  };
  document.querySelectorAll('[data-preview-on], [data-preview-seg], [data-preview-sel], [data-preview-num]').forEach((icon) => {
    const row = icon.closest('.settings-row');
    if (icon !== row) on(icon, () => show(icon, null));
    const sw = row.querySelector('input.switch');
    if (sw) on(sw, () => show(icon, null));
    const sel = row.querySelector('select');
    if (sel && icon.dataset.previewSel) {
      on(sel, () => show(icon, null));
      sel.addEventListener('change', () => open && show(icon, null));
    }
    const num = row.querySelector('input[type=range], input[type=number]');
    if (num && icon.dataset.previewNum) {
      on(num, () => show(icon, null));
      num.addEventListener('input', () => open && show(icon, null));
    }
    if (icon.dataset.previewSeg) row.querySelectorAll('.segment').forEach((b) => on(b, () => show(icon, b)));
  });

  // Capture phase: this runs before the document handler that closes the settings page, so Escape
  // closes the picture first and the page on the next press.
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && open) {
      const icon = current;
      hide();
      // Louis's text tooltip hides on mouseleave; it would otherwise stay up with the picture gone.
      if (icon) icon.dispatchEvent(new Event('mouseleave'));
      e.stopPropagation();
    }
  }, true);
  window.addEventListener('resize', hide);
  const scrollable = document.querySelector('.settings-scrollable');
  if (scrollable) scrollable.addEventListener('scroll', hide, { passive: true });
}
