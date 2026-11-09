// The picture that appears when the info icon of a setting, or one of its choice buttons, is
// hovered or focused: an isometric block diorama of what the setting does (src/gui/images/previews3d,
// made by scripts/previews3d). Only settings whose icon carries data-preview-on/-off (a switch) or
// data-preview-seg (a segmented control, one picture per button value) get one.

const BASE = 'images/previews3d/';
const WIDTH = 240; // the picture's CSS width; the popover adds its padding
const POPOVER_W = WIDTH + 24;
const GAP = 8; // px between the popover and the row
const MARGIN = 8; // px between the popover and the viewport edge

// File name (no extension) for the icon's setting in its current state, or for `button`, a choice
// button the pointer is on. Null when the setting has no picture for that state.
export function previewName(icon, button) {
  const row = icon.closest('.settings-row');
  if (!row) return null;
  if (icon.dataset.previewSeg) {
    const b = button || row.querySelector('.segment.active');
    return b && b.dataset.v ? `${icon.dataset.previewSeg}-${b.dataset.v}` : null;
  }
  const sw = row.querySelector('input.switch');
  return (sw && sw.checked ? icon.dataset.previewOn : icon.dataset.previewOff) || null;
}

export const captionKey = (name) => `preview_${name.replace(/[^a-z0-9]+/gi, '_')}`;

// Top-left corner for a popover of size `pop` next to `row`, all in viewport px. Beside the row on the
// side of the window with more room, so it never covers the row's own control; above or below it when
// the row spans the window.
export function place(row, pop, vw, vh) {
  const clampY = (y) => Math.max(MARGIN, Math.min(y, vh - pop.h - MARGIN));
  const right = row.right + GAP;
  const left = row.left - GAP - pop.w;
  const fitsRight = right + pop.w <= vw - MARGIN;
  const fitsLeft = left >= MARGIN;
  const middle = clampY(row.top + (row.bottom - row.top) / 2 - pop.h / 2);
  const rowFirst = (row.left + row.right) / 2 < vw / 2 ? [fitsRight && right, fitsLeft && left] : [fitsLeft && left, fitsRight && right];
  const x = rowFirst.find((v) => v !== false);
  if (x !== undefined) return { left: x, top: middle };
  // Full-width row: below it, else above it. In a window too short for either, on the side with more room.
  const x0 = Math.max(MARGIN, Math.min(row.left, vw - pop.w - MARGIN));
  const below = row.bottom + GAP;
  const above = row.top - GAP - pop.h;
  if (below + pop.h <= vh - MARGIN) return { left: x0, top: below };
  if (above >= MARGIN) return { left: x0, top: above };
  return { left: x0, top: vh - row.bottom > row.top ? clampY(below) : clampY(above) };
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
  img.loading = 'lazy';
  img.decoding = 'async';
  img.width = WIDTH;
  img.height = (WIDTH * 3) / 4;
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
    const row = icon.closest('.settings-row').getBoundingClientRect();
    const h = pop.getBoundingClientRect().height || (WIDTH * 3) / 4 + 52;
    const at = place(row, { w: POPOVER_W, h }, window.innerWidth, window.innerHeight);
    pop.style.left = `${at.left}px`;
    pop.style.top = `${at.top}px`;
    open = true;
  };

  document.querySelectorAll('.tooltip-icon[data-preview-on], .tooltip-icon[data-preview-seg]').forEach((icon) => {
    for (const ev of ['mouseenter', 'focus']) icon.addEventListener(ev, () => show(icon, null));
    for (const ev of ['mouseleave', 'blur']) icon.addEventListener(ev, hide);
    const row = icon.closest('.settings-row');
    const sw = row.querySelector('input.switch');
    if (sw) {
      sw.addEventListener('focus', () => show(icon, null));
      sw.addEventListener('blur', hide);
    }
    if (!icon.dataset.previewSeg) return;
    row.querySelectorAll('.segment').forEach((b) => {
      for (const ev of ['mouseenter', 'focus']) b.addEventListener(ev, () => show(icon, b));
      for (const ev of ['mouseleave', 'blur']) b.addEventListener(ev, hide);
    });
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
