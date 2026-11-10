// How the map draws a piece, in one place: the map (bbox.js, in its own frame) and the legend
// (meld-features.js) both read these, so a legend swatch cannot disagree with the map. A plain
// script (the map frame has no modules); a test imports it for the same effect.
globalThis.arnisPieceStyle = {
    colors: {
        queued: '#8a8a8a', start: '#fecc44', retry: '#fecc44', done: '#7bd864',
        skipped: '#7bd864', failed: '#fa7878', stopped: '#ff9f43', planned: '#5aa9ff',
        excluded: '#8a8a8a', room: '#5c5c5c'
    },
    // Line patterns: a piece's state, a margin piece (ring), room to grow, the halo, a stretch line.
    dash: { queued: '4 4', start: '8 8', retry: '8 8', stopped: '10 5 2 5', ring: '3 3', room: '2 4', halo: '2 6', stretch: '8 6' },
    // The selection, while it is drawn (dashed) and once it is placed.
    selection: '#fecc44',
    selectionDash: '10, 10',
    // The outline of a drawn shape that cuts the plan.
    mask: '#ffea00'
};

// The rectangle around drawn shapes (rings of [lat, lng]) as [south, west, north, east]: the area
// One World builds when it is not built in pieces.
globalThis.arnisShapesRect = (shapes) => {
  const lat = shapes.flat().map((p) => p[0]);
  const lng = shapes.flat().map((p) => p[1]);
  return [Math.min(...lat), Math.min(...lng), Math.max(...lat), Math.max(...lng)];
};

// A failed piece's card (with its Open log link) stays while the pointer is on the card, on the
// piece (leftAt null), or left the piece less than `grace` ms ago: time to reach the card across
// the neighbouring piece, whose own card is held back meanwhile.
globalThis.arnisCardHold = (overCard, leftAt, now, grace = 900) => overCard || leftAt === null || now - leftAt < grace;

// A piece layer removed while its card is open gets no mouseout: stop its poll, make it let go,
// and return the card now held (a dead piece must not keep holding back the others' cards).
globalThis.arnisCardDrop = (item, held) => {
    clearTimeout(item.closing);
    item.leftAt = 0;
    return held === item ? null : held;
};

// Whether a One World's overlays refit the map: another world, or an Open that asked for it.
globalThis.arnisShouldFit = (path, fittedPath, refit) => path !== fittedPath || Boolean(refit);

// Snap to Pieces while a selection is dragged: lengths in degrees move in whole regions (`side`).
// `span` is a size (at least one region, its sign kept); `shift` is a move.
globalThis.arnisSnap = {
    span: (d, side) => (Math.round(d / side) || (d < 0 ? -1 : 1)) * side,
    shift: (d, side) => Math.round(d / side) * side
};

// The cells worth drawing now: those that touch `view` ([south, west, north, east]), at most `cap`
// of them. `over` says more were in view than the cap allows (zoomed out); the map then draws
// the job's outline instead (`arnisCellsExtent`).
globalThis.arnisCellsInView = (pieces, view, cap = 2000) => {
    const cells = [];
    for (const p of pieces) {
        const b = p.bounds;
        if (b[2] < view[0] || b[0] > view[2] || b[3] < view[1] || b[1] > view[3]) continue;
        if (cells.length >= cap) return { cells, over: true };
        cells.push(p);
    }
    return { cells, over: false };
};

// [south, west, north, east] around the cells that are built (not the grey ones outside the outline
// or the room to grow); null when there are none.
globalThis.arnisCellsExtent = (pieces) => {
    let e = null;
    for (const p of pieces) {
        if (p.state === 'excluded' || p.state === 'room') continue;
        const b = p.bounds;
        e = e ? [Math.min(e[0], b[0]), Math.min(e[1], b[1]), Math.max(e[2], b[2]), Math.max(e[3], b[3])] : b.slice();
    }
    return e;
};

// A button that acts only when held for `ms` (pointer, or Space/Enter); letting go earlier does
// nothing. `.is-holding` is on while it is held, for the fill (CSS, timed to the same `ms`). Used
// by Start's hold (main.js) and the map's Clear selection (bbox.js). `blocked()` says the button
// cannot be used now; a hidden panel starts nothing either.
globalThis.arnisHold = (button, done, ms, blocked = () => button.disabled) => {
    let timer = 0;
    const stop = () => {
        clearTimeout(timer);
        timer = 0;
        button.classList.remove('is-holding');
    };
    // Only the primary button holds.
    const start = (e) => {
        if (blocked() || timer || (e && e.button)) return;
        button.classList.add('is-holding');
        timer = setTimeout(() => {
            stop();
            if (!blocked() && !button.closest('[hidden]')) done();
        }, ms);
    };
    const isKey = (e) => e.key === ' ' || e.key === 'Enter';
    button.addEventListener('pointerdown', start);
    ['pointerup', 'pointerleave', 'pointercancel', 'blur'].forEach((type) => button.addEventListener(type, stop));
    button.addEventListener('keydown', (e) => {
        if (!isKey(e)) return;
        e.preventDefault();
        if (!e.repeat) start();
    });
    button.addEventListener('keyup', (e) => {
        if (!isKey(e)) return;
        e.preventDefault();
        stop();
    });
};

// The map's one balance slider, -100..100. At the centre both layers are at their default; to the
// left the Minecraft world overlay fades (0 at the far left), to the right the cell overlay (grid,
// counts, labels, the darkening under them) fades (0 at the far right). `world` is the world
// overlay's own default opacity (0..1); both results are opacities 0..1.
globalThis.arnisBalance = (balance, world = 0.7) => ({
    world: world * (balance < 0 ? (100 + balance) / 100 : 1),
    cells: balance > 0 ? (100 - balance) / 100 : 1
});

// How long Clear selection is held, in ms (the fill in map-controls.css runs for the same time).
globalThis.arnisClearHoldMs = 800;
