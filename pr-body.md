## What this fixes

`clip_water_ring_to_bbox` assumes the ring it receives is already a closed loop.
When ring assembly leaves a fragment open (for example a multipolygon relation
whose member way was not loaded), the "ensure closed" step inside the function
connects the last point back to the first with a straight chord. Sutherland-Hodgman
then clips that chord-closed shape, the downstream closure checks accept it, and the
area floods as a straight-edged triangular or rectangular "water wedge" cutting
across the map.

## The change

Add an explicit closed-loop guard at the top of the function via a small
`ring_is_closed` helper. A ring with fewer than three points, or whose first and
last node are neither the same id nor within one block of each other, returns
`None`. Sibling rings that are properly closed still render, so the result is
partial water rather than a wedge. The closure test matches the one already used by
ring merging (same endpoint id, or endpoints within one block).

No behavior change for rings that are already closed. Only open fragments, which
previously produced the wedge artifact, are now dropped.

## Tests

Adds unit tests covering an open fragment (dropped), a shared-id ring (kept), a
coincident-endpoint ring (kept), and a degenerate ring (dropped).

`cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` all pass.

## Screenshots

Same area in each image: left half is before (an open ring chord-closes into a
straight-edged water wedge), right half is after this fix (the open fragment is
dropped and the water renders correctly).

<!-- drag image 1 here -->

<!-- drag image 2 here -->
