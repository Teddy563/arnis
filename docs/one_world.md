# One World

One World is a second generation mode for Java Edition. Instead of a fresh
world per selected area, every run extends one persistent world. Areas
generated at different times join without gaps or seams, and the world stays
aligned to real-world coordinates.

- CLI: `arnis --one-world --output-dir <saves folder> [--world-name "My City"] --bbox ...`
- GUI: Settings > World > **One World**. The world is `Arnis One World`, or,
  with Custom World Name on, whatever the pencil picks. The pencil never
  renames a world on disk; it chooses which One World the next area goes into.
  A status line under the format toggle shows the world and its area count, and
  the map shows every generated area as an overlay.

The first run creates the world, every later run with the same name extends
it. Java and Earth only.

## How it works

### One frame for the whole world

Every area uses the same isotropic Web Mercator projection, pinned to the world:

```
k = scale * cos(origin_lat)
x = R * (lon - origin_lon) * k
z = -(merc_y(lat) - merc_y(origin_lat)) * k        (north is -z)
```

`origin_lat`, `origin_lon` and `scale` are written to the manifest
(`arnis_one_world.json`, next to `level.dat`) when the world is created, from
the centre of the first request. Block `(0, 0)` is the origin, and a lat/lon
always maps to the same block.

Web Mercator is conformal, and its axes are separable (x depends only on
longitude, z only on latitude), so a block rectangle is exactly a lat/lon
rectangle. Every fetch (OpenStreetMap, elevation, land cover, canopy) can run on
the geographic bbox of the blocks it fills, and the per-area preview PNGs sit on
the GUI map without warping. The cost is scale drift: a block is `1/scale`
metres at the origin latitude, and `cos(origin_lat) / cos(lat)` times that
elsewhere, about 2% per 100 km north or south at mid-latitudes. A run whose
area is off by more than 25% says so. The world is one continuous plane, so
Earth's curvature leaves no gaps.

Requests beyond 85 degrees latitude, or past the Minecraft world border in the
world's frame, are refused before anything is written.

`--projection web_mercator` used to be anisotropic (only x had the `cos` factor)
and was refused by `validate_args`. It is fixed and usable on its own again,
with the origin at the bbox centre.

### Chunk-aligned areas

`projection::snap_bbox_to_chunks` grows the request outward to whole chunks in
the world frame and inverse-projects the result, which the run then uses
everywhere. Every chunk belongs to exactly one generation, so an area can be
written into existing region files without merging half-chunks.

### Writing into existing region files

`RegionWriteMode::Merge` (`world_editor/java.rs`) opens `r.X.Z.mca` if it exists,
creates it empty otherwise, and writes only the chunks inside the run's
rectangle. Nothing is written outside the area; Minecraft generates the rest
from the superflat settings in `level.dat`, which match the filler plane. A
region file that cannot be read fails the run instead of being replaced, since
it holds other areas.

The bundled region template is never used here: its placeholder chunks carry
another region's positions, and Minecraft refuses to load the ones a merge
leaves standing ("chunk is in the wrong location"). Worlds created by the first
build of this feature have such chunks; they are dropped once, under the lock,
on the next run (manifest version 1 to 2).

Rewritten chunks are also removed from `entities/` and `poi/`, so entities
Minecraft already moved out of the chunk do not linger over the new blocks.

### Seams

- **Geometry**: one projection and origin, floor semantics for negative
  coordinates.
- **Ground grids**: one cell per block, sized from the block rectangle instead
  of `geo_distance`. Providers sample a bbox edge to edge, so the fetch bbox runs
  from the first to the last block centre, and rows are re-spaced from equal
  latitude steps to equal Mercator steps in place (`grid_ops`). Every block is
  sampled at its centre. Elevation, land cover and canopy are fetched with a
  margin (`ground_pad_blocks`, at least 96 blocks) and cropped, so the smoothing
  passes see the same neighbourhood on both sides of a seam.
- **Metres to Y**: one mapping for the whole world, stored in the manifest when
  the world is created (see [Build height](#build-height)). Every area uses it
  (`AffinePolicy::Fixed`), so the same height is the same Y everywhere.
- **Elevation source**: a new world pins the top zoom of the Mapterhorn pyramid
  for its frame (`elevation_zoom`; one cell per block gives the same zoom
  anywhere in the frame, z16 at scale 1 in mid-latitudes). Every area and piece
  samples from it and fetches every pyramid level, without the spread probe that
  lets a large selection skip a level when its 16 sample tiles miss the data. When
  those tiles cannot be fetched, the area warns and falls back like an ordinary
  run (its own zoom, then AWS, then flat ground), which may leave a step at its
  seams. Worlds without the field (older, or Legacy Terrain, which is AWS only)
  keep choosing per area. The outlier gate, the anomaly filter and the
  land-cover smoothing have fixed or frame-fixed parameters, so they need no pin.
- **Elements across a seam**: OSM ways and Overture footprints are clipped to
  the area plus 64 blocks, so a building on the edge is built whole on both
  sides; writes outside the area are dropped.
- **Climate and biomes** are read at the world origin, so neighbouring areas
  never land on different sides of a Köppen boundary or biome band. The
  experimental `--climate-mode per-position` reads them per block instead, from
  the block's place in the world frame, so areas still agree where they meet;
  pass it to every area of the world.
- **Determinism**: the two unseeded RNG sites (recycling barrel loot, item frame
  side) are seeded now, and the OSM tile archive emits nodes in id order instead
  of `HashMap` order, which changed which of two overlapping trees won a block
  from run to run. Identical runs now produce identical blocks.
- **World seed** (experimental `--seed`): mixed into every element and
  coordinate seeded choice, so it stays seam-safe. The manifest keeps the
  first area's seed (none by default) and later areas and pieces build with it;
  caves follow it when `--cave-seed` is not given.
- **Map ids**: signage decal maps continue after the world's last map id; the
  world map item and branding map are placed for the first area only.

### Overlap

Chunks inside a new area are always generated again, also where an older area
or Minecraft itself already made them. That keeps every chunk the product of one
run and lets a newer Arnis rebuild an old area. Because it also replaces
anything a player built there, the count of existing chunks is read from the
region file headers before the run: the CLI prints it and the GUI asks for
confirmation. Areas the new one fully covers are dropped from the manifest along
with their previews.

### What the manifest fixes

Refused with a message naming the mismatch: world scale, ground level, terrain
on/off. Taken from the manifest: the elevation source (and its pinned zoom) and the build height.
Forced: rotation 0, Web Mercator, no Voxy LOD cache, Mapillary facades as
blocks, no preset facades, map preview on, no Luanti. The GUI greys and pins
these rows and puts the user's own values back when One World is turned off.
Everything else may differ per area.

### Build height

A One World can end up holding the Alps next to a coastal city, so every new
world gets the extended build height pack (Y -2032 to 2031) when it is created,
whatever the setting says, and a mapping that has room for all land on Earth
(`ElevationAffine::whole_earth`):

- The Dead Sea shore (-430 m) is Y -2014, one section above the lowest terrain
  Y the pack allows, so water carved there keeps its bed.
- Above it, one block per metre at scale 1 (`scale` blocks per metre in
  general), the same proportion as horizontally. Sea level is Y -1584, Munich
  about Y -1064.
- The top 800 blocks under the terrain ceiling (Y 2016) are a soft top: from
  2800 m up, `asinh` compresses heights more the higher they are, so Everest
  lands exactly on the ceiling and nothing on land is cut flat. At the knee the
  slope matches the straight part. Mont Blanc (4808 m) is about Y 1800.
- Only sea floor below -430 m is clamped, and it lies under water.

Scales of 0.4 and below fit Everest without the soft top. The knee and its
width are stored in the manifest, so later builds keep using the same curve.
Slopes are measured through the curve (`ElevationData::soft_top_stretch`), so a
rock face above the knee still counts as steep for the rock and snow rules.

Numbers the manifest stores at creation (origin, scale, knee, width) are
rounded to a few decimals first: serde_json reads short decimals back exactly,
but not every `f64` it writes, and the first run must use the same numbers as
every later one.

The world's base is the Dead Sea, far below most areas, so bedrock, the
superflat plane and the montane tree check follow each area's own lowest point
instead (`ground::area_floor_for`). Bedrock sits 64 blocks under it, as in an
ordinary world, which keeps `--fillground` columns short. Minecraft generates the
land outside the areas as a superflat plane at the first area's lowest point.

Worlds created before manifest version 3 keep the build height they were made
with and the mapping their first area settled (vanilla band, up to 96 blocks of
headroom below that area). A later area outside that band is flattened where it
clamps, and the run says so when that affects more than 0.5% of it.

### Locking

`prepare` takes the world's `session.lock` before it reads the manifest and
holds it until the world is written, in the GUI and the CLI. A world Minecraft
has open is refused. Java locks the file with `fcntl`, so on Unix Arnis does the
same (on macOS a second, `flock`-style lock from the same process would block
it); on Windows it uses `LockFileEx`. Probing a lock this process already holds
is skipped, because closing any handle to the file would drop a POSIX lock.

### Failure handling

A world created by a run that fails is removed again, by the GUI and the CLI.
A failed run reports the error in the GUI instead of "Done!". The area is recorded only
after its regions and preview are written; the elevation mapping is stored
earlier, so a rerun of a failed first area stays on the same mapping.

Moving the spawn with a marker also moves the player into the overworld, in case
they logged out in another dimension. The map's "Copy coordinates" gives a
`/spreadplayers` command for a One World, which lands on the top block, since
terrain can be anywhere between Y -2014 and 2016.

Every run moves `LastPlayed` in `level.dat` to now, so the world is listed first
in Minecraft's world list.

### Manifest (`arnis_one_world.json`)

```json
{
  "version": 3,
  "created_with": "arnis 3.2.0",
  "created_at": 1789000000,
  "origin_lat": 48.1372, "origin_lon": 11.5755,
  "scale": 1.0, "ground_level": -62,
  "terrain": true, "disable_height_limit": true, "aws_only_elevation": false,
  "elevation": {
    "min_height_m": -430.0, "blocks_per_meter": 1.0, "ground_level": -2014,
    "soft_top": { "knee_m": 2800.0, "width_blocks": 194.4 }
  },
  "elevation_zoom": 16,
  "next_area_id": 3,
  "areas": [
    { "id": 1, "generated_at": 1789000000, "arnis_version": "3.2.0",
      "min_x": -800, "min_z": -560, "max_x": 799, "max_z": 559,
      "min_lat": 48.132, "min_lon": 11.564, "max_lat": 48.142, "max_lon": 11.587,
      "preview": "arnis_one_world/previews/area-1.png" }
  ]
}
```

Manifests are validated on load. Preview paths are only followed inside
`arnis_one_world/previews`, since a manifest can come with a downloaded world.
`metadata.json` describes the union of all areas.

## Large areas

One run holds the whole selection in memory, which bounds how large a One World
area can be. A job built in pieces is bounded by the piece instead:

```
arnis --one-world --output-dir <saves> --bbox ... --unit-regions 4 --one-world-workers auto
```

- `--unit-regions N` cuts the selection into pieces of at most N x N regions
  (one region is 512 x 512 blocks; N from 1 to 64, default 4). The cut lines
  sit on a lattice anchored at block 0 of the world, so every piece owns whole
  region files, and two jobs over overlapping selections cut the same lines.
  Pieces are built centre first.
- `--one-world-workers auto|N` builds that many pieces at once, each in its own
  process (a run of the same executable). The job's threads, memory and
  downloads are split between them. Either flag alone starts a job; the other
  takes its default (one worker, 4 x 4 regions).
- `--plan-units N` prints the cut as one JSON line and exits without touching
  the world.
- GUI: Settings > Advanced Features > **Parallel Workers** and **Piece Size**,
  available with One World on. The bar follows the whole job and the status
  line counts finished pieces.

**Auto sizing.** `auto` plans on 75% of the cores (`--cpu-target` or
`--threads` override that) and the free memory less a reserve of 10% of RAM, at
least 2 GB (`--ram-budget-mb` overrides the reading). It runs as many pieces as
both allow, at least 4 threads each, never more than 6 pieces or more than there
are. A piece's memory is estimated from its size and the scale until pieces
finish; after that the largest peak they reported lowers the count if needed.
A fixed N splits the threads, memory and downloads one run would take N ways.

**Coordinator.** The process you start holds the world's lock for the whole job
and writes everything that belongs to the world as a whole once, after the last
piece: the manifest record, `level.dat`, the map id counter, `metadata.json`
and the area preview, stitched from the pieces' previews. Pieces write only
their own chunks and signage maps, with the world's elevation mapping fixed in
the manifest before the first one starts. A piece dies with its coordinator
(a Job Object on Windows, a closed stdin pipe on Unix), so closing the window or
stopping the CLI stops the whole job.

**Margin.** Each piece builds 32 blocks past its edges (never past the
selection) and writes only its own chunks, so a tree rooted in the neighbour's
ground still spreads its crown into the piece, and takes the tile path one run
of the whole selection would take. A one-region piece does about a quarter more
work; a four-region piece about 6%. On a forest at scale 1 (640 x 576 blocks,
four one-region pieces) the blocks that differ from one run fell from 540,081
(2.2% fewer leaves) to 662, around the one-block ground steps of the elevation
baseline (Known limitations, 4).

**Resume.** A job keeps its state in `arnis_one_world/jobs/<rect>_n<N>/`: one
`done-<piece>.json` per finished piece. Running the same selection with the
same N again skips those and builds the rest; once a piece is done, a failed or
stopped job keeps the world instead of removing it. A failed piece is retried
twice when the cause may pass (network trouble, a crash without a message).
The job folder is removed when the job completes.

**Known limitations of pieces.**

1. **Bedrock floor per piece**: like separate areas (limitation 9 below), each
   piece follows its own lowest point, so bedrock and the deepslate line can
   step at a piece seam. With `--caves --cave-datum-y` bedrock drops to the
   datum wherever a piece's own floor sits above it, so pieces share one floor
   and caves, pools, rivers and geodes continue across seams. On a hilly 640 x
   672 block area at scale 1 (datum -64, four one-region pieces) the blocks
   below the surface that differ from a single run fell from 16,761,846 to
   about 40; what is left are stone-variant and ore blobs within a few blocks
   of a seam, next to the surface baseline of limitation 2.
2. **Seam-crossing geometry**: tunnels and polygons that cross a piece seam are
   clipped per piece; about 0.02% of blocks differ from one run over the same
   area.
3. **Map id stride**: each piece gets 65,536 signage map ids; a piece needing
   more fails, and a job whose pieces would run past the map id space is
   refused (use a larger `--unit-regions`).
4. **Unix** process handling (stdin watch, process groups) has not been run;
   jobs were validated on Windows.

## Known limitations

1. **Facade photo panels and preset facades** write a resource pack per run
   that would replace the previous one, so they are off. Fix: merge each run's
   textures and models into the existing `resources.zip`.
2. **Voxy LOD cache** is rebuilt from one run's regions, so it is off. Fix:
   add sections to the existing database, or rebuild from all regions.
3. **World map item** shows the first area only. Fix: re-render it from all
   previews after each run.
4. **Elevation providers**: worlds created before `elevation_zoom` was pinned
   can still step where a large area skipped a sparse high-resolution level that
   its neighbour used, or fell back to AWS. A pinned world stops at the
   resolution steps of the dataset itself, which a single run has too. Under
   0.2% of the columns near a seam differ from a single run, mostly by one block
   (each area samples its own fetch grid; see Validation).
5. **Water depth tiers** are computed per water body inside the area, so a lake
   cut by an area edge can get a small underwater step. Fix: compute tiers on
   the padded grid before cropping.
6. **Very large areas** whose padded grid would exceed the elevation grid cap
   (about 268 km² at scale 1) are fetched without the margin and without
   block-centre alignment.
7. **Longitude is not wrapped**: an area more than 180 degrees of longitude from
   the origin is placed the long way round the globe, or refused at the world
   border.
8. **Region palettes** can list the same blocks in a different order between
   identical runs (pre-existing, from how property blocks are deduplicated). The
   blocks themselves are identical.
9. **Caves** follow each area's bedrock plane, so where two neighbouring areas
   got different floors their caves and deepslate line do not line up at the
   seam. Within an area they match across tiles as usual. `--cave-datum-y`
   pins one depth frame for every area and piece (the manifest keeps the
   first area's value, and its `--cave-seed`), lowers bedrock to the datum
   where an area's own floor sits above it, and plans pools, rivers and geodes
   past the area's edge so a feature crossing it is whole once both sides are
   built. An area whose lowest point needs bedrock below the datum keeps its
   own, and its caves step at the seam, so pick the lowest bedrock of the
   region or below.

## Where to look

- `src/one_world.rs`: manifest, `prepare`, `record_area`, `existing_chunks`.
- `src/work_units.rs` (the cut), `src/scale/` (coordinator, piece processes,
  worker sizing).
- `src/projection/`: `WebMercatorProjection`, `ProjectionSpec`,
  `snap_bbox_to_chunks`.
- `src/ground.rs` (`GroundFrame`), `src/grid_ops.rs`,
  `src/elevation/postprocess.rs` (`ElevationAffine`, `AffinePolicy`).
- `src/world_editor/java.rs`: `RegionWriteMode`, merge and repair helpers.
- `src/world_utils.rs`: `SessionLock`, `world_is_locked`.
- `src/gui.rs`: `gui_one_world_info`, `gui_one_world_overlap`,
  `gui_get_one_world_overlays`; `src/gui/js/main.js` (`refreshOneWorldState`,
  `prepareOneWorldRun`); `src/gui/js/bbox.js` (`oneWorldOverlays`).

## Validation

- Ordinary generation (One World off) produces the same block hash as `main`
  in the default, rotated, flat and terrain-only modes, once `main`'s own
  node-order nondeterminism is fixed on both sides. The whole-Earth mapping
  left it unchanged too (default, rotated, flat, terrain-only, a compressed
  Matterhorn area and the extended build height, against the previous build).
- Three CLI runs into one world on the Arnis test area (the area, its eastern
  neighbour, a smaller area across their seam): no chunk missing, no misplaced
  chunk, the overlap reported and rebuilt. Surface heights across the area seam
  match 86.6% exactly and 6.9% within one block, in line with ordinary chunk
  boundaries inside an area.
- A new world with Munich, its eastern neighbour and the Matterhorn 300 km away:
  the pack is enabled in `level.dat`, the superflat plane sits at Munich's
  lowest point (Y -1078), Munich spans Y -1090 to -969 with bedrock at -1152,
  and the Matterhorn reaches Y 1772 with bedrock at 1072, snow-capped like an
  ordinary world of the same area. The seam between the Munich areas is in the
  range of the interior chunk boundaries next to it. 10 to 14 s per area.
- Separate areas against one run over their union, same frame, terrain only,
  ground height in 32 columns either side of the seam:
  - Bucharest, a 640 x 336 block area and a 2144 x 1792 (or 6128 x 5568)
    neighbour: 8 of 21,504 columns differ by 1 to 2 blocks, the seam step
    differs in 1 of 336 rows. Romania has only the 30 m floor, so the pin
    changes nothing there; the large neighbour probes past z16 and z15 but
    reads the same tiles.
  - Bucegi, a 640 x 672 area and a 2432 x 3920 neighbour: 28 columns
    differ, 26 of them by one block (bedrock under trees aside, limitation 9).
  - Slovenia, an area on 1 m LiDAR at the edge of a coverage gap
    (46.460,16.437,46.466,16.447) and a 6592 x 5024 neighbour inside the gap:
    before, the neighbour's probes all missed and it sampled the strip next to
    the seam at z14: 3,560 of 21,504 columns on its side differed and the seam
    step differed in 103 of 672 rows. With the pinned zoom: 0 and 0; 56
    columns differ by one block inside the small area.
  - A Mapterhorn outage (unreachable proxy) builds a flat area, with and
    without the pin; the pinned fetch only adds a warning.
- Ordinary runs are NBT-identical to the previous build (Bucharest terrain
  only, Slovenia default mode).
- A world made by the previous build (vanilla height, version 2) is extended
  with its own mapping, no pack, and stays version 2.
- The same area generated into two fresh worlds gives identical blocks.
- A second process holding `session.lock` makes the run refuse without touching
  the manifest; the test for it runs on Windows and Linux CI.
- A copy of a world from the first build is repaired (7607 stray chunks
  dropped) and extended correctly.
- Unit tests cover the projection, snapping, manifest life cycle, locks, region
  merge and repair, row remap and crop, ground fetch plan, affine policies and
  the whole-Earth mapping.
- The Unix lock code has not run on macOS. The GUI flow was reviewed but not
  clicked through.
