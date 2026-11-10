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
