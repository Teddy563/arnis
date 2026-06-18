# arnis (Teddy563 fork) - v2.9.1

A maintenance release on top of 2.9.0. Three things:

1. Each Meld cell can now read its own slice of a shared OSM tile cache directly, with no per-cell merge step.
2. A water rendering artifact (the "wedge") that could flood a triangle of a cell with water is fixed.
3. The Overture building fetch skips work it does not need.

All changes are additive. A normal single-world run (no `--osm-tile-dir`, buildings on) behaves as before.

## New: read OSM straight from a tile cache (`--osm-tile-dir`)

Meld keeps OSM on a stable web mercator grid (one JSON file per z11 tile, named `osm_g1_z{Z}_{X}_{Y}.json`). Before this release, Meld merged each cell's covering tiles into a single per-cell file and passed it with `--file`. That merge was the slow "assembling" step, and it fed Arnis a superset that had to be clipped back down.

Now:

```
arnis --output-dir ./world --bbox <cell-bbox> --osm-tile-dir ./cache/osm --osm-tile-z 11
```

Arnis computes the z11 tiles that overlap `--bbox`, loads each `osm_g1_z11_{X}_{Y}.json`, concatenates and de-duplicates the elements by (type, id), and feeds that to the normal parser. The orchestrator fills the directory once and hands the same directory to every cell, so there is no per-cell assembly at all. `--osm-tile-dir` is mutually exclusive with `--file`. A tile that is not on disk is skipped.

`--prewarm-overture` is the building-side companion: `arnis --bbox <region-bbox> --prewarm-overture` fetches and caches the Overture partitions and STAC index for the bbox and exits, so later cells read buildings from disk.

## Fixed: triangle and rectangle water floods (the "wedge")

Symptom: a cell would render a large triangle or rectangle of water with a hard, dead-straight diagonal edge cutting across terrain.

Cause: a water multipolygon (a lake or river bank) is an outline made of member ways. When the water body extends beyond the tiles loaded for a cell, some member ways are missing, so the assembled outer ring is left open (a broken loop). The bbox clip then closed that open ring by connecting its two loose ends with a straight chord, and the even-odd scanline fill flooded everything on one side of the chord. That straight chord is the diagonal edge.

Fix: `clip_water_ring_to_bbox` now checks that its input ring is actually closed (first node matches the last by id, or the two are within one block). If it is open, the function returns nothing, so the broken outline is dropped instead of being closed with a fake straight edge. Rings that are properly closed are unaffected, so legitimate water still renders. Both water paths (`element_processing/water_areas.rs` and `land_cover_osm_water_override.rs`) call this function, so both are covered.

Verified on real 1:10 data: a wedge cell went from 135,600 to 4,555 water blocks (the triangle is gone, the real river is kept), and a clean river cell came out byte identical (no legitimate water lost).

## Changed: Overture buildings are gated and cached

- The Overture Maps building fetch and render now run only when buildings are enabled. A `--no-buildings` run no longer downloads building partitions it will not use. Measured on a roads-and-ground 1:10 cell: about 28.8 s down to about 4.2 s.
- The Overture STAC index is cached locally with a 7 day TTL, and the per request byte ranges are cached under the Arnis cache root, so repeated cells over the same area read from disk with no extra network. The earlier full-partition lock that could stall a whole batch on one slow download was removed.

## Files touched

`src/args.rs`, `src/osm_parser.rs`, `src/main.rs`, `src/clipping.rs`, `src/land_cover_osm_water_override.rs`, `src/overture.rs`, `src/gui.rs`, `Cargo.toml`.
