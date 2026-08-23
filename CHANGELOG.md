# Changelog - Meld fork

All releases of the Meld fork of louis-e/arnis. Follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format.

This fork is the generator behind [Meld](https://github.com/Teddy563/meld): Meld slices a real-world
selection into region-aligned cells and runs one arnis process per cell; everything the fork adds
(shared master origin, global elevation band, tile-invariant rendering, tile-mode OSM/terrain reads,
tree packs, terrain shaping, caves) exists so that many independently generated cells merge into one
seamless world. Every flag is additive — omit it and upstream behaviour is preserved.

Starting with 2.9.0 the fork tracks the upstream Arnis version number; earlier entries used an internal 1.8.x sequence.

## [Unreleased]

Two silent failures, both found while triaging Discord reports against Meld's
logs. Neither changes generated output when things are working.

### Fixed

- **Overture survives a stale release pointer.** Overture keeps only the newest
  few data releases online, so when the release the fork resolves goes stale it
  goes stale for every cell at once — a cliff, not a slope. Across 19,018 cell
  logs, 6,420 runs fetched Overture and 16 failed outright, all sixteen in one
  render, all `404` on a retired release. A cached STAC index that is merely old
  is now preferred to no index at all: the refresh window is a week and the
  partitions it points at outlive that comfortably, but a failed refresh used to
  propagate and kill the whole step while a perfectly usable index sat on disk
  unread. The release that last worked is also remembered next to the cache and
  tried before the compile-time fallback, so that constant — a date, and dates
  rot — stops being load-bearing. Release attempts 3 → 4.
- **The Bake lighting checkbox in the desktop GUI has never done anything.**
  `gui_start_generation` accepts `bake_lighting_enabled`, the front end has
  always sent it, and the `Args` built from it set `bake_lighting: false`
  unconditionally — so a desktop-Arnis world could not be pre-lit however the
  user set the toggle. Upstream wires this; the fork stopped, and
  `#[allow(unused_variables)]` on the function suppressed exactly the warning
  that would have caught it. Wired, and the allow removed.

## [3.1.2] - 2026-08-21

Optional B_Linear output. `--region-format blinear` writes Leaf's own region
container directly instead of Anvil, so a world destined for a Leaf server needs
no conversion pass afterwards. Default is unchanged: omit the flag and every
render is Anvil exactly as before.

**Server support is narrow.** A `.b_linear` world opens only in Leaf 1.21.11
(June 2026 builds) or newer, or any Leaf 26.x, with
`misc.region-format.format-name: B_LINEAR`. Paper, Purpur, older Leaf and the
vanilla client cannot read it, and no `.mca` original is kept alongside.

Measured against Anvil on the same bboxes and seeds:

| | Anvil | B_Linear |
|---|---|---|
| dense city, 25 regions | 113.36 MB | **30.57 MB** (3.71x) |
| terrain + caves + baked light, per region | 4.69 MiB | **1.20 MiB** (3.91x) |
| 224 regions, streaming eviction | 990.2 MB, peak RAM 1701 MB | **245.7 MB** (4.03x), peak RAM 1810 MB |

### Added

- **`--region-format {mca,blinear}` and `--blinear-level 1..22`.** B_Linear v3
  groups a region's 1024 chunks into 16 buckets of 64 and zstd-compresses each
  bucket as one frame, replacing Anvil's per-chunk zlib in 4 KiB sectors. Chunk
  NBT is untouched by the swap: the container branches where the Anvil writer
  already hands its serialized chunk to fastanvil, so both containers carry the
  same world and convert between each other losslessly. Leaf verifies the
  superblock, the version byte and every chunk's xxh32 (seed `0x0721`, which it
  hardcodes) strictly, so all three are written to its contract.

  The container is a field on the existing Java path rather than a fourth world
  format, which leaves parallel tiles, stream-to-disk eviction, the golden-hash
  gate and every block-entity schema untouched. Regions are built in memory and
  published by rename, so a killed run leaves whole files or none.

  Not available in the GUI: it writes into `.minecraft/saves`, where the client
  has to be able to open the world. `--map-item` is skipped for these worlds
  because its renderer reads regions back through fastanvil, which only speaks
  Anvil.

### Verified

- Equivalence is established in-process, because generated worlds cannot be
  compared byte-for-byte at all: palette ordering follows `HashMap` iteration
  order, so two Anvil runs of one seed differ in every chunk while describing
  the same world. The same NBT written through both sinks reads back identical
  from both files, `ARNIS_BLOCK_HASH` agrees across both containers and both
  eviction modes, and decoding both worlds block by block found **27.3 million
  non-air positions identical across 22,528 chunks**, with no chunk in a slot
  its own coordinates do not name.

- A world generated this way was booted on Leaf 1.21.11, force-loaded, saved and
  re-read: no hash failures, and the regions still verify after the server
  rewrote them.

## [3.1.1] - 2026-08-18

Farmland texturing got much cheaper. Nothing about the world changes: every render
in this release is byte-identical to 3.1.0, verified by world hash.

Measured on the farmland-dense bbox upstream used to report the regression
(`52.520,5.600,52.550,5.660`, Flevoland, 61 farmland polygons, one process,
stream-to-disk off), best of three runs each:

| | 3.1.0 | 3.1.1 |
|---|---|---|
| peak memory, field texture on | 4929 MB | **1333 MB** |
| generation time, field texture on | 24.5 s | **11.8 s** |

### Fixed

- **Crop blocks allocated their NBT per block.** A crop is always `{age: "<0..=7>"}`,
  so a whole field shares one of eight possible compounds, but `place_crop` built a
  fresh `HashMap`, two `String`s and an `Arc` for every block placed, and the world
  editor then held that per-block allocation until save. The eight compounds are now
  built once and shared. This alone accounted for about 3.8 GB of the peak.

- **Section palettes re-derived the same answer 4096 times.** Building a section's
  palette formatted each cell's property compound to a string to key the lookup, so a
  field of crops paid thousands of string allocations per section to rediscover the
  same eight entries. An identity check now sits in front of the content lookup. The
  content lookup is still authoritative, so two equal compounds from different
  allocations collapse into one palette entry exactly as before; there is a test for
  that specifically, because getting it wrong would silently change every world's
  palette.

- **Field parcel layout took a lock and three trigonometric calls per block.**
  `bearing_at` was called once per block but only ever asked about the centre of a
  192 x 192 domain, so roughly 36,000 consecutive blocks asked the identical question
  and each one took an `RwLock` read contended across every rendering thread, a hash
  lookup, and an `atan2` plus a `sin` and a `cos`. The answer is now memoised per
  thread and per lattice cell, versioned against the grid so a cached bearing cannot
  outlive the grid it came from.

## [3.1.0] - 2026-08-18

Upstream port wave 3: what the fork takes from `louis-e/arnis` `7f8236f..3918513a`
(upstream v3.0.10 → v3.1.0, 90 commits across 17 pull requests). Triage of every
upstream change, with the reason for each take/adapt/skip, lives in
`.light-meld-docs/UPSTREAM-TRIAGE.tsv`; the plan and its gates are in
`.light-meld-docs/UPSTREAM-PORT-PLAN-3.1.0-WAVE3.mdx`.

Batches are gated. The first two produce a **byte-identical render** — same world, same
`block_hash`. After that each output-changing batch names the difference it intends and
attributes it by building the change alone, and every batch renders once at **scale 0.1**
to prove objects still appear at Meld's default scale.

### Added
- **A golden world-hash regression harness** (`scripts/golden_hash.sh`) with five committed
  OSM fixtures covering deliberately different tagging regimes — a dense European old town,
  a US suburban tract, Manhattan towers, a medina, and sparse subarctic. Generation was
  previously ungated: the only determinism instrument was an env-var hash check with no
  committed baseline, so a regression could only be found by looking at a finished world.
  `--update` rebaselines. The fixtures are converted to Overpass JSON at runtime by
  `scripts/osm_xml_to_overpass_json.py`, because this fork's `--file` reads Arnis JSON only.
- **`--scale` validation.** NaN, the infinities, zero, negatives and absurd values are now
  rejected by the parser with a real message instead of reaching the fetch stage and
  producing a hung or empty cell. The accepted range is **0.01 to 4.0** — the floor is
  Meld's, not upstream's 0.05, because Meld's planet renders live down there.

### Fixed
- **The update check pointed at upstream.** Every fork build polled `louis-e/arnis` and
  nagged the user to "update" to a release containing none of the fork's features, whose
  asset names Meld's generator updater cannot consume. Now points at `Teddy563/arnis`.
- **Overture returned nothing at all.** Release discovery went through
  `stac.overturemaps.org/catalog.json`, which stopped resolving, and the hardcoded fallback
  release had since been retired — so every STAC request 404'd and no Overture building was
  ever placed. Releases are now discovered from the bucket listing, newest first, with the
  revision compared numerically and at most three tried.
- **Truncated Overpass responses were accepted as complete.** Overpass streams its output,
  so a query that runs out of time or memory after printing has started still closes the
  JSON and appends a remark. The elements present are real and the ones it never reached
  are simply absent, so a half-finished area parsed exactly like a finished one and built a
  world with most of its content missing. Such a response now fails over to the next server.
- **The extended-height datapack froze time.** Both overlay entries carried the deprecated
  `formats` key, which since 1.21.9 makes the game reject the whole overlays section; the
  world falls back to the legacy data tree, which on 1.21.11 has no `timelines` field, so
  the overworld has no day timeline and `/time set` is a no-op. The key is now emitted only
  for a target positively verified as pre-82 — and Meld, which passes no `--mc-version`, is
  no longer such a target.
- **Terrain blur was shifted half a cell.** `create_gaussian_kernel` centred an odd-sized
  kernel at `size / 2.0` while the caller indexed it with integer `size / 2`, so every
  terrain render was smoothed off-centre on both axes.
- **Dams, weirs and culverts were drawn as open water.** `waterway=dam|weir|dock|…`
  describes a structure, not a channel, and outlining one ran a canal along its crest. A
  culvert (`tunnel=*`) cut a channel straight through the bank above it — only
  `layer=-1|-2|-3` was recognised, as literal strings. A mistyped `width=*` could ask for a
  channel wide enough to hang generation, and is now clamped to 128.
- **`--bbox` panicked on malformed input.** Empty strings, too few or too many fields,
  non-numeric fields, NaN and the infinities all unwrap-panicked; they now return an error.
  Meld builds that string on every invocation.
- **Barriers tagged `material=wood`** render as oak fence, alongside the existing
  `material=metal` rule.
- **Tile editors never received the world scale.** `editor.scale` read 1.0 inside every
  tiled element pass while the sequential path saw the real value. Output-neutral today,
  since nothing reads it, and now the two paths cannot silently disagree.
- **Software-rendering environment variables are no longer forced.**
  `LIBGL_ALWAYS_SOFTWARE` and `GALLIUM_DRIVER` are only set when the user has not, which
  fixes `EGL_BAD_PARAMETER` on Wayland/AMD setups.
- Unresolvable OSM way node references are counted and reported once per run. A way that
  loses a node is silently shortened; for an area that breaks the ring, and an unclosed
  ring fills to nothing, so the polygon vanished with no diagnostic.
- CI: apt retry/timeout wrapper on the Linux setup action and job timeouts, so a sick
  Ubuntu mirror fails fast rather than hanging a job. The release tag guard now checks
  `tauri.conf.json` as well as `Cargo.toml` — checking only one is how the crate reached
  3.0.10 while the bundle still said 3.0.7.

### Deliberately not taken
Recorded here so a later port does not "complete" them by accident. Full reasoning in the
plan document.
- **`OBJECT_SKIP_SCALE = 0.3`**, which makes upstream skip every OSM and Overture object
  below scale 0.3. Meld's default project scale is 0.1 and its entire purpose is rendering
  countries at 1:10 *with* buildings, roads and rails, so adopting it would silently empty
  every default render. A test asserts `skip_objects()` stays false at low scale so a future
  port fails there first. The fork already has the right shape of that idea scoped per
  feature, in `--props-min-scale`.
- **The building realism overhaul (PR #1269) as a whole.** A measured three-way merge puts
  70 conflict regions and 4,384 conflicted lines across eight files, `buildings.rs` alone at
  27.7% of the merged file. Both sides rewrote the same functions; this is a
  re-implementation, not a cherry-pick. Individual slices remain candidates.
- **Image signage (PR #1272).** It allocates map ids per world and writes `data/map_N.dat`,
  but Meld merges cells by copying `region/`, `poi/`, `entities/`, `datapacks/` and
  `level.dat` — never `data/`. Every cell would restart ids at 0 and the merge would drop
  them all, leaving blank item frames throughout. It also has no scale gate, and at 1:10 a
  128×128 map decal covers 1.28 km of ground.
- **The `--terrain` deprecation notice**, which prints once per process. Meld runs one
  process per cell, so a country render would print it thousands of times.
- **The country-scale deep floor and section-snap work**, and the rest of the tunnel,
  shoreline and building work, are deferred rather than refused — they need the seam grid
  and the golden harness this release introduces.

## [3.0.10] - 2026-08-16

### Removed
- **The stadium 3D archetype** (`src/models_3d/custom/stadium.rs`): the generated stadium
  GLB kept landing on ordinary football/soccer fields, so it is deleted outright — tagged
  stadiums now render procedurally like any other area. The plane archetype and the
  3DMR/Wikidata model pipelines are unchanged.

## [3.0.7] - 2026-08-13

Upstream port wave 1: everything the fork is taking from `louis-e/arnis`
`af521c9..17cdd62` (upstream v3.0.0 → 2026-08-10). Landed in batches; every batch except
the last is gated on producing a **byte-identical render** — same world, same
`block_hash`, just faster or safer. Only two changes here are meant to be visible, and
each was attributed to its own commit by building it alone. Triage of all 31 upstream
commits, with the reason for each take/adapt/skip, lives in
`.light-meld-docs/UPSTREAM-TRIAGE.tsv`.

### Fixed
- **Spawn Y was wrong in every merged world.** The post-generation spawn pass rebuilt its
  own coordinate box from the bbox text with no master origin, so for any cell whose bbox
  is not its origin — which is every cell after the first in a Meld run — the ground lookup
  addressed the wrong part of the grid. It now takes the same box the rest of the pipeline
  uses. Measured on an origin-offset cell: `SpawnY` −53 → −55, blocks untouched.
- **A wind generator that isn't a wind turbine no longer gets a 150 m tower.** Any
  `power=generator` + `generator:source=wind` placed the full freestanding turbine
  schematic, so the two 3.2 m vertical-axis rotors mounted at level 2 of the Eiffel Tower
  became two towers. Skipped when the tags say mounted or micro: positive `min_height` or
  `level`, `location=roof|rooftop`, `generator:type=vertical_axis`, or `rotor:diameter`
  under 10 m. Real wind farms are untouched — Fântânele-Cogealac renders to the same hash
  before and after.
- **Pedestrian bridges stopped rendering as motorway viaducts.** A module deck is a road
  deck: wide, pillared, load-bearing. A bridge group made only of footways, cycleways,
  paths, bridleways or steps still got one whenever its style resolved to Beam. The group
  now needs at least one vehicular member. Downscaled worlds (`--scale ≤ 0.3`) keep their
  flat one-block deck exactly as before.
- **A corrupt schematic now fails by name instead of stamping in the wrong place.** Sponge
  requires exactly `Width*Height*Length` palette indices; a stream that ran long or short
  folded the surplus into out-of-range coordinates. Checked in all three decoders this fork
  carries, including the cave-pack loader — the one that reads a user-droppable directory.
  Volumes past `i32::MAX` are rejected up front. Every bundled asset was swept before the
  check was made fatal: 479 tree schematics, 113 cave schematics in 18 families, and the
  prop models, with zero trips.
- **The tiled path grew a list nothing read under memory pressure.** With region eviction
  active the subway carve already runs in-tile, and the post-merge carve is skipped — but
  the point list was still accumulated for the whole world, on the exact path chosen
  because memory is tight.

### Performance
- **Relation ring assembly is no longer cubic.** `merge_way_segments` tracked merged
  segments in a `Vec<usize>` and called `contains()` from inside the nested pair loop, so
  an *n*-segment relation paid an O(n) scan per pair on top of the O(n²) search — and the
  function recurses. A per-index bool marker makes the bookkeeping O(1) with the same merge
  order and provably identical output. Every multipolygon we assemble benefits: coastlines,
  the Danube, building outer/inner rings, the OSM water override.
- **Tile assignment memoizes relation member AABBs.** Assigning one relation to tiles
  re-walked every member way's nodes once per candidate tile. Member boxes are now computed
  once, unioned for the region range, then tested per tile. Same tiles selected.
- **Hot tag tests stopped allocating.** Three comparisons (park relations, the castle wall
  pick, the shelter amenity) built a throwaway `String` per call just to compare it.
- **Column probes resolve their chunk once.** The tree canopy's roof check asked for one
  block per Y level, redoing the region/chunk/section lookups at every step; it now walks
  the resolved chunk's sections from the top. The Y range is intersected with the *runtime*
  world bounds rather than clamped end-by-end against the vanilla constants — clamping each
  end folds a fully out-of-world range onto a boundary block and reports a hit nobody asked
  for, and the constants would blind the walk to whole sections under a tall or lowered
  height profile.

### Changed
- `serde` 1.0.228 → 1.0.229, and a `quinn-proto` security bump from the upstream dependabot
  group.
- Nix: the `dda-voxelize-0.2.0-alpha.1` git dependency is hash-pinned (our flake could not
  build reproducibly without it) and `flake.lock` follows upstream's nixpkgs bump.

### Added
- **`--mode geo-terrain|geo-only|terrain-only`** — the documented way to say what a run
  generates, and a real terrain-only path for the CLI. `terrain-only` skips *both* object
  sources: no Overpass query, no `--osm-tile-dir` read, and no Overture fetch, so `--overture`
  has no effect in that mode (`--file` / `--osm-tile-dir` / `--save-json-file` are ignored,
  with a note on stderr). Since Overture is ~93% of a cell's wall time, terrain-only is
  dramatically cheaper than a full run.

  **This fork keeps terrain OFF by default — upstream turns it on.** `--mode` has no default
  here: with neither flag a run renders flat ground, exactly as every archived command and
  stored project expects. `--terrain` is unchanged and fully supported (`--terrain` ==
  `--mode geo-terrain`, omitting it == `--mode geo-only`); it is only hidden from `--help`
  now that `--mode` is the documented spelling. `--terrain` together with `--mode geo-only`
  is a contradiction and is rejected up front. No existing render moves by a block.
- `NOTICE` — the Apache-2.0 attribution file. We redistribute built binaries of this fork,
  so section 4 requires it. Upstream text verbatim plus a short section naming this
  repository as a derivative work.

## [3.0.6] - 2026-08-10

Field reports, chased to root causes. Three real bugs — crops destroyed by baked lighting,
large map areas rendering as an outline around untouched ground, and a datapack warning on
1.21.9+ servers — plus four newly verified Minecraft versions. Everything here is a fix or
an additive flag; defaults are otherwise unchanged.

### Fixed
- **Baked lighting destroyed every crop it lit.** Crops were missing from the
  light-transparency list, so the skylight column scan treated wheat as an opaque block,
  stopped at it, and wrote `SkyLight 0` into the crop's own cell. With `isLightOn` set, the
  client believes that: the crop renders black, then Minecraft destroys it, because a crop
  needs light 8. Measured on a real farmland world before the fix: **3,211,825 of
  3,211,825 crop cells at SkyLight 0**; after, they bake at 15. Reported as "the crops
  would be black and then break". The same omission covered melon/pumpkin stems, cocoa,
  torchflower and pitcher crops, and big dripleaf, all now transparent.
- **Large OSM areas drew an outline with nothing inside.** `flood_fill_area` refused any
  polygon whose *bounding box* passed 25M blocks, but the callers paint the polygon's edge
  before they ask for the fill — so a big `natural=sand` dune field at 1:1 came out as a
  sand border around dirt and gravel. Oversized polygons now go through a scanline fill
  that needs no bitmap (memory is the output alone), the budget counts the cells actually
  covered rather than the bounding box, and `natural` skips a polygon's edge entirely when
  its fill did not fit — no border is better than a border around nothing. A big-bbox,
  small-area shape (a river bank, an L, a sparse coastline) now fills where it used to be
  dropped for its bounding box alone.
- **`pack.mcmeta` warning on 1.21.9 and newer.** Pack format 82 deprecated the overlay
  `formats` key in favour of `min_format`/`max_format`, and servers logged
  `Overlay "overlay_attributes" key formats is deprecated starting from pack format 82` on
  every start. For a target whose format is verified to be 82 or above, the deprecated key
  is now dropped; older targets keep it, since it is the only overlay selector they
  understand.

### Added
- **`--props-min-scale <SCALE>`** (default `0.35`). Schematic props are fixed-size builds,
  so a scaled-down world does not scale them: at 1:10 a parked crane is the size of a
  district. Below this scale they are skipped, with one line saying so. `0` places them at
  any scale, and an explicit `--props` list is still honoured above the threshold.
- **Verified rows for 1.21.10 (4556), 1.21.11 (4671), 26.1 (4786) and 26.1.1 (4788)**, and
  the missing **26.2 datapack format (107.1)** — so extended build height works on 26.2
  instead of being refused. Every number was read out of Mojang's own `version.json` inside
  the client jar (`world_version` and `pack_version.data_*`), which is exact and needs no
  generated world; the method was cross-checked against the rows previously verified from
  real `level.dat` files (1.21.5 → 4325, 26.1.2 → 4790 and format 101.1) and matched.
  `assets/mc_versions.json` documents the source per row, and a test now asserts every
  26.x row carries a verified pack format.

## [3.0.5] - 2026-08-06

Configurable, version-aware build height. A world's vertical range is now derived from
the terrain it actually holds and declared by a generated datapack, instead of a fixed
4064-block preset — and the target Minecraft version is a real input rather than a
hardcoded constant. Every flag is additive: without them the generator behaves exactly as
3.0.4 did.

### Added
- **`HeightProfile`** (`src/height_profile.rs`) — the single object defining a world's
  `min_y` / `height` / datum / vertical scale / target version. The datapack writer, the
  chunk writer and the reported geometry all read it; nothing computes its own. Every
  engine invariant (16-alignment, the -2032..2031 range, `min_y + height <= 2032`, the
  signed-byte section index) is enforced in one `validate()`, and every violation is a
  refusal rather than a clamp.
- **Fitting rules**: the smallest legal height rather than the maximum, explicit
  `--height-headroom` / `--height-underroom`, and vertical compression only as a last
  resort — always reported, never silent.
- **`--mc-version`** and a checked-in capability table (`assets/mc_versions.json`) giving
  the `DataVersion`, extended-height support and chunk layout per version. Its rule is
  that no value is written from memory: every row records where its numbers were read
  from, and a test enforces that. An unknown version is refused with the list of known
  ones instead of being approximated.
- **`--min-y` / `--max-y`** to set the world's floor and ceiling explicitly. Checked, not
  clamped: a ceiling under the terrain's peak or a floor over its base is refused and says
  what it would have cost.
- **`--water-carve-clearance max|auto|BLOCKS`** — seam-critical for tiled generation, see
  Fixed.

### Fixed
- **The floor was a lie.** The bundled pack declared `min_y: -2032` while every write went
  through `y.clamp(MIN_Y, MAX_Y)` with `MIN_Y` a compile-time `-64`, so extended height
  only ever extended UPWARD and terrain below vanilla's floor could not be generated at
  any setting. The write path now clamps to the profile's real range, and both
  under-terrain fill paths reach the world's floor, so a declared basement is solid rock
  rather than void. Measured: with `--height-underroom 96`, ground reaches Y -160.
- **The terrain datum depended on how much water a tile held.** `Ground::new_enabled`
  raises the datum to clear the deepest water carve and MEASURED that depth from the
  tile's own land cover, so an inland tile sat at Y -62 and a coastal one at Y -57 — a
  Y-cliff along every coastline crossing a cell border in a tiled world. A fixed
  clearance makes every tile reserve the same room; `max` is exact rather than
  speculative because the carve depth is bounded by `MAX_WATER_DEPTH`.
- **Two DataVersions in one world.** `assets/minecraft/region.template` is a full
  1024-chunk region baked at a fixed version; chunks the generator never overwrote kept
  it, so a finished world held both the writer's version and the template's (4790 and
  3955 in the test that found it) and Minecraft ran its DataFixer over part of it. The
  template is now restamped as it is written.
- **The datapack was written in the wrong schema for 26.x, so the world would not open
  at all** — "Errors in currently selected data packs prevented the world from loading",
  with `Failed to read pack file/arnis_tall metadata` in the log. The bundled pack uses
  the 1.21.x metadata shape (integer `pack_format`, `supported_formats`, `[major, minor]`
  arrays, dimension_type split across overlays); 26.x replaced that with a single DECIMAL
  format and rejects the old one. The pack is now emitted in the TARGET version's schema:
  a modern target gets one dimension_type already in that era's attributes/timelines form
  and a mcmeta carrying the decimal format, with no overlays. The format number is
  verified like every other constant here (101.1, read from a pack the 26.1.2 client
  loads), and a modern version with no verified number refuses extended height rather
  than guessing — which is why 26.2 is currently refused.
- **level.dat's version is no longer restamped.** An intermediate fix set it to the target
  version; that field describes the format the FILE is in, and the bundled level.dat is a
  1.21.x-era file, so claiming otherwise made Minecraft skip the DataFixer that migrates
  it and the world then failed even in Safe Mode. Chunk DataVersion stamping stays,
  because those the writer really does produce in the modern format. A DataVersion may
  only claim what a file structurally IS.
- A fitted world always covers the vanilla band, since the writer still emits the vanilla
  column of sections; a narrower dimension would ship chunks holding blocks above its own
  ceiling. When the terrain needs nothing beyond vanilla the result IS vanilla geometry
  and no datapack is written at all — no experimental-features prompt, and no unremovable
  pack, for nothing.

### Changed
- The datapack is generated from the profile rather than copied, and in the shape the
  target version's codec accepts. The bundled JSON templates stay, because they encode
  schema differences across 1.21.4-1.21.10, the 1.21.11 era and 26.1.x that must not be
  invented; the writer picks the one matching the target and rewrites only `min_y` /
  `height` / `logical_height`.
- The pack is written in `generate_world_with_options` — the one point the CLI and the GUI
  share — before the first chunk, replacing two install sites that both ran before the
  terrain range was known.
- Refusals, not clamps: a pre-1.17 target asking for extended height is told the version
  floor and its two real options; a pre-1.18 target is refused because that chunk layout
  is a different writer.

### Verified
**Loaded in Minecraft 26.1.2.** A 4096x4096-block Yosemite run at 1:4, generated as four
independent processes (the tiled case) and merged the way Meld merges: all four declared
the same world (Y -80..831), chose the same datum, stamped the same DataVersion, kept
every section inside the declared world, and put terrain at Y 559 — 240 blocks above
vanilla's ceiling. The merged world opens in game with the generated height datapack
active. 342 unit tests pass.

Worth stating plainly: the two datapack bugs above were invisible to every structural
check — the files were internally consistent and said exactly what the profile said. Only
launching the game surfaced them, and the client log named the failing codec directly.
That is the check that has to happen before an extended-height change is called done.

## [3.0.4] - 2026-07-26

The Farmlands release: open land becomes real agricultural country. Farmland, grassland,
and the untagged satellite plains split into rotated field parcels with monoculture crop
plots, hedged by dirt tracks, dotted with rocks, bushes, and hay — all seam-safe (every
decision is a pure function of world coordinates) and additive: omit the new flags and
default runs stay byte-identical.

### Added

- **Field parcels.** `--field-mix <name=pct>` splits `landuse=farmland` into a weighted
  mix of five styles — `coarse` (packed mud / rooted dirt / locked path patches with dead
  bushes), `plains` (vanilla-density grass), `flower` (2-3 wildflower species per plot),
  `farm` (tilled crops), `moss` (overgrown) — laid out as rectangular parcels with
  dirt-track boundaries and noise-textured interiors. The map divides into orientation
  domains with noise-warped borders; each domain picks an angle and a layout (long
  strips or blocky plots), and **aligns to the dominant nearby road** so fields follow
  the street network like real cadastre. Parcel sizes are defined in real metres and
  track the map scale; `--field-scale <25-400>` zooms the pattern. Farmland polygons
  feather into the surrounding grassland over a dithered two-cell edge.
- **Real crop plots.** `--farm-crops <name=pct>` weights seven monoculture plot kinds:
  wheat, potato, carrot, beetroot, sunflower (planted rows), pumpkin (grass/coarse
  mosaic patch), and fallow (resting bare ground). Each field grows ONE crop at ONE
  growth stage — neighbouring fields ripen differently, younger spots and bird-sown
  stray-crop patches break up the carpet, and wheat/fallow plots carry hay-bale
  bundles. Sunflower fields cluster in the low, open parts of the map (terrain-aware)
  and thin out uphill. Crop-planted farmland never decays in-game; fallow uses the
  locked bare palette so nothing reverts to dirt.
- **Texture beyond farmland.** `--grass-texture` extends the pattern to OSM grassland
  (meadow / grass / greenfield / orchard / village_green, plus `natural=grassland` and
  `meadow` cover) with its own mix via `--grass-mix`; `--land-texture` covers the land
  OSM never mapped, keyed by ESA satellite land cover — cropland takes `--land-mix`
  (falls back to the farmland mix), grassland takes the grass profile, and villages
  (`landuse=residential`) get grassy ground instead of wheat. Every textured style
  carries vanilla-density cover: short grass, ferns, two-block large ferns, tall grass,
  ten wildflower species, sunflowers, dead bushes, and moss carpet.
- **Rock & bush scatter.** `--rocks` / `--bushes` drop bundled schematics — 8
  andesite/tuff rock formations and 60 bushes (10 species × 6 shapes) — at random
  rotations across farm, grass, and untagged land: bushes in ~5% and the much rarer
  rocks in ~2% of 16×16 chunks, stamped only on natural dry ground (never onto rivers,
  lakes, roads, or tilled farmland). `--rock-density` / `--bush-density` are deprecated
  and ignored.
- **New blocks (ids 439-449).** `beetroots`, `pumpkin`, `sunflower` (double plant),
  `packed_mud`, `oxeye_daisy`, `cornflower`, `allium`, `orange_tulip`, `pink_tulip`,
  `lily_of_the_valley` — wired through Java block states, Bedrock, and Luanti maps.

### Fixed

- **Snow: peaks mode ignores flat terrain.** `--snow-mode peaks` places no snow when the
  terrain's vertical relief is under 150 m, so flat lowland farmland no longer gets
  stray snow speckle; genuine mountains still cap.
- **Floating plants over water.** The floating-vegetation sweep now covers the new
  species (sunflower halves, the six new flowers, moss carpet, azalea), so nothing
  hovers over rivers and lakes.

## [3.0.3] - 2026-07-15

Water and cave rendering fixes plus organic climate boundaries. Every change is seam-safe (a pure function of world coordinates) and additive; default runs stay byte-identical away from the affected water/cave surfaces.

### Fixed

- **Water colour bands.** The water biome no longer uses the warm, lukewarm, and cold ocean temperature variants, which gave inland lakes and wide rivers visible colour lines and wrongly tinted temperate inland water. Non-freezing water is now river, ocean, or deep_ocean (all one shared tint) and freezing water is the frozen trio, so a water body reads as a single uniform colour. The ocean vs deep_ocean split is kept for mob spawning and is colour identical, so it adds no seam.
- **Thin land and road lines across water.** Untagged roads and tracks crossing water at small scale are now drowned instead of painting a 1-block causeway straight across a lake or river, and interior terrain ridges that poked above the water surface are submerged instead of rendering as a thin grass line. Real shores, islands, and tagged bridges are unaffected. The road drown is kept in sync with the road mask so no dry gap is left where a crossing is removed.
- **Cave lava seam trench.** The deep lava sea left a 2-block-wide air trench at every cell and region seam because its containment check treated an out-of-bbox neighbour as open, so both tiles skipped their boundary columns. The check now ignores out-of-bbox neighbours, so both tiles fill the seam and the lava is continuous. Interior generation is byte identical.

### Changed

- **Climate boundaries render as organic blobs.** The Koppen lookup is domain warped by a smooth low-frequency noise field before sampling the 0.1 degree grid, so the blocky rectangular climate edges bend into natural blobs. It is a pure function of latitude and longitude, so it stays seam-safe across tiles, and the world and the climate preview change together.

## [3.0.2] - 2026-07-14

Additive feature release for the Meld climate, heightmap, tree, and seam work. Every change is additive and seam-safe; omit the new flags and behaviour is unchanged, and default runs stay byte-identical.

### Added

- **Per-position Koppen climate.** Climate (biome tint and arid/polar surface blocks) is now sampled per block from the bundled Koppen grid instead of once per cell, so a large tiled region varies smoothly across the map instead of snapping to one climate per cell. New `Climate::at(lat, lon)` and `Ground::climate_at(x, z)` drive both the biome and surface-palette paths.
- **`--climate-map <PREFIX>`.** Early-exit render mode: writes a top-down Koppen climate PNG for the bbox (one colour per grouped climate, the same grouping generation uses) plus a `CLIMATEMAP {json}` stats line. Pure function of the bbox, lines up with a geographic overlay.
- **Tree size popularity weights.** New `--tree-size-weights small=..,big=..,giant=..` sets relative per-tier popularity (100 = the pack default share, 0 = off, 200 = about double) by reweighting the existing tile-invariant size roll. Scale gates still apply (Giant only at 1:1, tiny maps never place tall/giant). Legacy `--tree-sizes` stays as an on/off alias. Default weights run the original picker verbatim, so output is byte-identical.
- **`--prewarm-elevation`.** Early-exit that fills the exact per-tile elevation cache the generation cells read (Mapterhorn globally, or the regional/AWS provider the fork picks) without sampling a grid. Companion to `--prewarm-overture`; lets Meld bake a region offline so cells never rate-limit the tile server.
- **`--elevation-map <PREFIX>`.** Early-exit that renders the heightmap for the bbox to a PNG using the real provider stack (so the preview matches the world), hillshade or grayscale, with an `ELEVMAP {json}` line carrying min/max metres and bbox for a map overlay.

### Fixed

- **Boundary props no longer clip at cell seams.** Node-placed prop schematics (wind turbine, lighthouse, crane, excavator, tractor, tombstone, and the rest) were assigned to exactly one strict tile with no halo, while area ways get the editor halo, so a prop straddling a cell boundary lost the voxels that crossed into the neighbour tile at merge. Nodes now get the same halo as area ways, so both cells stamp the prop; safe because every node handler places deterministically on world-absolute coordinates (idempotent, byte-identical writes).

## [3.0.1] - 2026-07-12

Upstream parity release: brings the fork to feature parity with **louis-e/arnis 3.0.0** (plus fork-only fixes upstream does not have), while keeping every Meld differentiator: tile-invariant rendering, shared master origin, `--road-detail`, the native cave engine, the scale-aware water/shore work, offline mode, and the master-origin elevation grid. Every feature below was ported faithfully and audited for cross-tile seam-safety by an adversarial review pass before shipping; the fork's #1 invariant is that independently generated cells merge into one seamless world.

### Added

- **New CLI commands and flags.** `--props all|none|<list>` (bundled 3D props per family), `--loot-table <json>` / `--dump-loot-table` (configurable chest loot), `--gamemode survival|creative|spectator`, `--world-time <ticks>`, `--map-item` (locked world map in the player inventory), and `--map-item-only` (add the map to an existing merged world in one post-pass, the Meld path).
- **Bundled 3D props at real OSM features.** Boats on open water, parked cars in car parks, cranes and excavators on construction sites, tractors on farmland, wind turbines at wind generators, lighthouses, fountains, playgrounds, cemetery tombstones, and the Starbase Pad 2 Starship landmark easter egg. Per-family toggle; omitting the flag keeps every family on.
- **Skyscraper facade taxonomy + OSM StyleHint.** True skyscrapers render in six distinct facades (glass curtain, glass with concrete corner pillars, glass in a concrete grid, contemporary concrete frame, horizontal-band modern, and stone/art-deco masonry) instead of two. A tag classifier reads `building:material` / `facade:material`, `roof:material=glass`, `historic` / `heritage` / `ref:nrhp` / `listed_status`, masonry and concrete material and cladding lists, `building:architecture`, and `start_date` (pre-1945 = masonry) to pin a tower's style, propagated across all parts of a multi-part (S3DB) building so untagged parts match.
- **Facade detailing.** A darker stone base course / plinth on normal buildings; full-glass storefront bays across commercial and hotel ground floors; string courses and a crown cornice on plain facades; coherent per-building window frames (bands, posts, hanging lanterns, French-balcony rails, stud buttons) on commercial, hotel and historic buildings; and a per-building window phase so blocks of buildings no longer share one citywide window grid.
- **S3DB multi-part buildings.** Outlines are suppressed only when their `building:part` members actually cover at least half of them, plus a spatial pass for relation-less S3DB; all parts of one building share a style seed so colour and roof stay coherent.
- **Furnished interiors, sport pitch markings, world settings.** Interior furniture with real beds, `leisure=pitch` line markings, and game mode / time of day written into `level.dat`.
- **Climate.** Koppen-driven biomes by real-world climate, plus climate-aware water: warm / lukewarm / cold / frozen oceans (with deep variants offshore), frozen rivers in polar and boreal regions, and tropical `sparse_jungle` shrubland.
- **Infrastructure.** Covered highway tunnels, street lamps along lit ways, electrified railway catenary, modular bridge deck schematics (flat low-detail bridges preserved), more OSM surface materials, `amenity=bicycle_parking` surfaces, `landuse=farmyard`, `aeroway=helipad` pads (light-gray pad, white ring, painted "H") for way and node helipads, and power line spans that hang straight between poles instead of hugging the terrain.
- **Trees.** Deliberately-mapped street trees may stand on plazas and sidewalks; trees no longer grow up through bridge decks; a canopy drapes over an adjacent low roof instead of being sliced flat at the building edge.
- **Mapterhorn global elevation** (default). Global terrarium tiles (GLO-30 floor, national LiDAR to z18) with pyramid-parent hole-proofing that structurally eliminates the broken-tile cliff artifacts of the legacy AWS source. Regional high-res providers (USGS, IGN France/Spain, Japan GSI) still win in their coverage; legacy AWS stays available via `--aws-only-elevation`, and a fork-only offline gate makes Mapterhorn honour `--offline`.

### Fixed

- **Stream-to-disk corruption guard.** Sign, entity and chest writers now bail on an already-flushed region like the block writer does, so large exports can no longer resurrect a stale region and truncate saved chunk data.
- **CLI spawn-Y.** The post-generation spawn-Y correction targets the actual world folder, so nested and desktop-output worlds get a correct spawn height.
- **Farmland irrigation** water is placed only where it sits in a basin, so it no longer runs downhill and washes out crops on sloped fields.
- **Helipad seam.** The pad renderer's aggregate rooftop-skip and the parked-helicopter prop (both keyed on cell-local state) were removed after the seam audit flagged them; the tile-invariant pad, ring and "H" remain.

### Notes

- Only the parked helicopter prop remains deferred: it needs a seam-safe region-ownership guard before it can be placed. The fork's residential window decorator is kept for houses; window frames apply only to the building types it does not cover, so the two never double-decorate.
- Block ids that collide with upstream were assigned at the fork's next-free ids rather than adopting upstream's palette wholesale, so existing fork worlds and assets are unaffected.

## [2.9.3] - 2026-07-04

Underground update: one cave system, done right. The experimental cave engine is gone; `--caves` now runs a from-scratch Rust port of Minecraft 1.21.8 cave worldgen, carved directly into the filled ground at generation time. This is the engine behind Meld 1.5.0 "Meld Depths" — Meld's Caves toggle passes `--caves` to every cell, and because every cave pass is a pure function of (seed, position), the caves line up across cell seams like the surface does.

### Added

- **`--caves` (cave worldgen).** Clean-room port of vanilla 1.21.8 cave generation: the noise density field (cheese caverns, spaghetti tunnels, entrance pockets, pillars, noodle worms) sampled with vanilla's 4x8x4 cell interpolation, plus the random-walk tunnel/ravine carvers. On top of the vanilla base: pool caves (multi-lobe, half-flooded, grand and coral-reef variants), snake rivers (long, downhill, up to 3 streams per source, breach into caves only while descending), a contained deep lava sea below y=-54 with obsidian/magma rims, the vanilla ore table plus three size tiers of stone-variant patches, and 8 cave biome themes (lush, dripstone, deep dark, mushroom, ice under mountains only, amethyst, volcanic at the bottom of the world, coral in water pools) covering about half the underground with plain-rock buffer strips between zones. Implies `--fillground`. `--vanilla-caves` is accepted as a legacy alias. Usage: `arnis --output-dir ./world --bbox <bbox> --terrain --caves`.
- **Cave asset packs.** A `cave-pack/` directory next to the arnis executable (or `--cave-asset-pack <DIR>`) supplies Sponge `.schem` formations - ice spikes, dripstone columns, amethyst clusters, clay pool basins, snow piles - stamped onto cave floors and ceilings, themed by biome zone, with block-states preserved and rotated. Formations sink into the ground, clip safely against walls, and never touch fluids. Without the directory, caves still generate fully (procedural decoration only).
- **Deterministic + seam-safe.** Every cave pass is a pure function of (seed, position): adjacent tiles generated independently carve the same caves at the seam.
- **`--cave-biomes <list>` (biome mix control).** Per-theme amounts as `name=percent` pairs (`lush=150,deepdark=0,amethyst=50`): 100 = the default share, 0 = theme off, 200 ≈ double its area (the percent shifts that theme's noise threshold on a log2 curve, so growth stays smooth and the field stays a pure function of seed + position — seam-safety unaffected). Omitting the flag reproduces the default distribution byte-for-byte; depth/terrain gates (volcanic bottom-only, ice under mountains, coral in pools) always apply.
- **`--cave-zone-map <prefix>` (layout preview).** Renders the biome zone layout for `--bbox` without generating a world: `<prefix>-upper.png` (y=-20 band) + `<prefix>-deep.png` (y=-48 band), transparent where the cave is plain rock, plus a `ZONEMAP {json}` stdout line with the measured share of every theme. Uses the exact zone picker, seed and `--cave-biomes` values generation uses, so the preview IS the layout the world gets. Meld drives this for its on-map cave-biome preview.

### Fixed (cave polish)

- **Deep dark is deep-only now.** Its depth gate moved from `y <= -18` (mid-cave) to `y <= -35` (below the deepslate line), so sculk belongs to the depths like vanilla instead of bleeding into the shallow/mid caves.
- **Stone-variant patches actually appear.** Diorite/granite/andesite/dirt were placed with absolute Y ranges (0-60), which land mostly above ground in low, valley-height real-world terrain and were pruned to near-nothing (a valley region measured diorite=4). They are now placed **surface-relative** (2-72 blocks below the local surface), so they fill the rock under every column at any terrain height — measured ~26k of each variant per region (evenly balanced), the vanilla "patches of diorite/granite/andesite everywhere" look.

### Removed

- **The old experimental cave engine** (`src/cave/`, ~3,900 lines) and its flags `--carver-caves` and `--cave-pack` (the cave.json biome-pack override). The stone-to-deepslate fillground transition it hosted moved to its own module (`src/deepslate.rs`, 123 lines) and behaves identically. Why: two cave systems meant double maintenance, and the experimental one produced artifacts the new engine's verification harness would never accept.
- **`--underilla`** (building-palette remap + `underilla_mask.bin` sidecar). Why: it existed solely for the external Paper+Underilla cave pipeline, which the native `--caves` engine supersedes (~100× faster, no Java server, no restore mask).
- Dead `ore_generation` module (157 lines). Why: only the deleted engine called it; `--caves` has its own vanilla ore table.

### Stats

Measured against the 2.9.2 release commit: **31 files, +6,178 / −169 lines** in `src/` + `examples/`. Breakdown: `src/caves/` + `src/deepslate.rs` = 4,024 lines (density-field port 402, noise 265, RNG 267, carvers 330, water features 404, formation engine 714, biome decoration 820, ores 238, pipeline 461, deepslate 123); verification tooling in `examples/` ≈ 1,900 lines (cavecheck region auditor, caveshape probe, mask preview) — the harness that gated every build on zero floating fluid / zero water-lava contact / zero ore-masking errors; the rest is wiring (flag unification, direct `--output-dir` world output, Underilla removal, 72 new block definitions for the coral/ice/snow/deep-dark biome themes).

## [2.9.2] - 2026-06-22

Two themes: load your own tree models, and shape the terrain. This release adds a pack-agnostic schematic tree system and two terrain knobs on top of the 2.9.1 generator. It ships no tree art; it places whatever pack directory you point it at.

### Added

- **`--tree-pack <DIR>` (schematic tree packs).** Points Arnis at a directory of Sponge `.schem` tree models plus a `region.json` manifest and places those models instead of the built-in procedural trees. The engine picks a realm by location, a community by terrain, then a weighted species, with a vanilla-tree blend mixed in, and stamps the models inside Arnis's own tree-spawn pass so density and clearings stay natural. Mutually optional: without the flag, the built-in procedural trees are used exactly as before. Usage: `arnis --output-dir ./world --bbox <bbox> --tree-pack ./packs/eur`.
- **`--tree-sizes <list>` (size by scale).** Comma list of `small,medium,big,tall,giant` choosing which height tiers may place. A disabled tier falls back to a smaller one rather than leaving a gap, and large tiers are gated off on small scales so a 1:N city does not sprout 40-block giants. Usage: `arnis ... --tree-pack ./packs/eur --tree-sizes small,medium,big`.
- **`--vertical-exaggeration <FACTOR>`.** Multiplies terrain height only, not the map footprint, so relief that flattens at small scales comes back; it auto-compresses to the build height. Default 1.0. Usage: `arnis ... --terrain --vertical-exaggeration 2.0`.
- **`--snow-mode off|realistic|peaks|manual` plus `--snow-percent <P>` / `--snow-y <Y>`.** Controls snow placement: off, the real latitude snow line, the top N percent of world height (`--snow-percent`), or above a fixed Y (`--snow-y`). Usage: `arnis ... --terrain --snow-mode peaks --snow-percent 15`.

### Fixed

- **Tree anchoring (float fix).** Trees anchor on slopes and at water edges instead of floating, never root in water, may overhang the bank with canopy, and never clip into buildings.
- **Small-scale water.** Scale-aware river and pool depth (a linear bowl below scale 0.5), line-river width scale-capped so 1:10 streams stay 3 blocks or under, and a more varied riverbed. Trees and leaves are swept off the water surface so the waterline stays clean.

### Changed / Engine

- **`--no-buildings` skips street furniture.** Also drops street-furniture nodes (lampposts, signs, traffic signals), not just building footprints.

## [2.9.1] - 2026-06-18

A maintenance release on top of 2.9.0. It lets a Meld orchestrator feed each cell its own slice of a shared OSM tile cache directly (no per-cell merge step), removes a water rendering artifact that could flood a triangle of a cell, and makes the Overture building fetch skip work it does not need.

### Added

- **`--osm-tile-dir <DIR>` plus `--osm-tile-z <Z>` (tile mode).** Reads OSM for `--bbox` straight from a directory of stable web mercator grid tiles (`osm_g1_z{Z}_{X}_{Y}.json`, default Z 11) instead of one merged `--file`. Arnis computes the tiles that overlap `--bbox`, loads each, and de-duplicates elements by (type, id), so an orchestrator can fill the directory once and hand the same directory to every cell with zero per-cell assembly. Mutually exclusive with `--file`; a missing tile is skipped. Usage: `arnis --output-dir ./world --bbox <cell-bbox> --osm-tile-dir ./cache/osm --osm-tile-z 11`.
- **`--prewarm-overture`.** Fetches and caches the Overture building partitions and STAC index for `--bbox`, then exits, so later parallel cells read the buildings from disk instead of each refetching them. Requires only `--bbox`. Usage: `arnis --bbox <region-bbox> --prewarm-overture`.

### Fixed

- **Triangle and rectangle water floods (the "wedge").** A water multipolygon whose member ways were not all present (for example a lake that extends beyond the loaded tiles) left its outer ring open. The bbox clip then closed that open ring with a straight chord and the scanline fill flooded the whole side of it with water, drawing a hard diagonal edge across the cell. `clip_water_ring_to_bbox` now rejects an open input ring (first node not matching last by id or within one block) and returns nothing, so a broken outline is dropped rather than closed with a fake edge. Sibling rings that are properly closed still render, so legitimate water is preserved. Verified on real data: a wedge cell dropped from 135,600 to 4,555 water blocks (triangle gone, real river kept) while a clean river cell was byte identical.

### Changed / Engine

- **Overture buildings gated on `--no-buildings`.** The Overture Maps building fetch and render now run only when buildings are enabled, so a `--no-buildings` run no longer downloads building partitions it will not use (measured: a roads-and-ground cell dropped from about 28.8 s to about 4.2 s).
- **Overture disk cache.** The STAC index is cached locally with a 7 day TTL, and per request byte ranges are cached under the Arnis cache root, so repeated cells over an area read the buildings from disk with no extra network. The earlier full partition lock that could stall a whole batch on one slow download was removed.

## [2.9.0] - 2026-06-16

A fork of louis-e/arnis 2.9.0 tuned for large parallel "Meld" generation: one orchestrator slices a region into many adjacent cells and bakes them into a single Minecraft world. This release pulls the fork up to the upstream 2.9.0 line (53 merged commits, in-process tile parallelization, stream-to-disk eviction) and adds the cross-tile machinery (shared origin, global elevation band, seam-free buildings, parallel-safe terrain fetch) that keeps those cells lining up block for block.

### Added

- **`--no-buildings` (alias `--no-structures`).** Drops all OSM buildings plus building-adjacent features (man_made, power, barriers, doors, advertising, historic, emergency, tourism) while keeping roads, rail, water, land cover and terrain, for a roads-and-ground-only Meld base layer; parking and some leisure/surface features are deliberately kept. Usage: `arnis --output-dir ./world --bbox <bbox> --no-buildings`.
- **`--tile-invariant-rendering` (alias `--seed`).** Makes a building that straddles two adjacent tiles render byte-identical in both, by reading pre-clip bounds and mixing the seed into every RNG stream. Bare flag means seed 1; omitting it keeps upstream behaviour. Usage: `arnis --output-dir ./world --bbox <cell-bbox> --seed 42`.
- **`--road-detail max|clean|compact`.** Trades road detail for legibility at low scale where footways, crossings and lane dividers stack into checker noise. `max` (default) is upstream-exact, `clean` is a cleanup pass for scale >= 0.7, `compact` keeps vehicle roads only and caps lanes to 2. Gates both the Overpass query and per-element render. Usage: `arnis --output-dir ./world --bbox <bbox> --scale 0.1 --road-detail compact`.
- **`--overpass-url`.** Overrides the Overpass endpoint(s) with a comma-separated, priority-ordered list that replaces the built-in public mirror pool, so a self-hosted instance can absorb large parallel batches without per-IP rate limits. Usage: `arnis --output-dir ./world --bbox <bbox> --overpass-url http://localhost:12345/api/interpreter`.
- **`--download-only` plus `--save-json-file`.** Fetches the OSM data for `--bbox` to a JSON file and exits, so a scheduler can pull a whole region's OSM once and feed it to many cells via `--file` instead of each cell tripping the public Overpass limit. Honours `--overpass-url` and `--road-detail`. Usage: `arnis --bbox <region-bbox> --download-only --save-json-file region.json`.
- **`--download-terrain-only`.** Warms the AWS terrain (elevation) tile cache for `--bbox` in one single-process pass (8 concurrent) and exits, so later parallel cells hit the cache instead of bursting S3. Requires only `--bbox`; exits 0 if all tiles cached, 2 if any failed. Usage: `arnis --bbox <region-bbox> --download-terrain-only`.
- **`--offline` (alias `--elevation-cache-only`).** Hard offline elevation: cache hits serve as usual, but a cache miss or corrupt tile returns an error instead of re-downloading (Arnis then falls back to flat ground), so a batch run never quietly hammers S3 or regional providers. Pair it with a prior `--download-terrain-only`. Usage: `arnis --output-dir ./world --bbox <bbox> --terrain --offline`.
- **`ARNIS_ELEV_ZOOM` env var.** Caps the terrain tile zoom for the whole run (clamped to the valid band) so elevation stays lighter and hole-free; a coarser zoom such as z13 still carries the full roughly 30 m signal while sidestepping the z14/z15 no-data holes and downloading far fewer tiles. No CLI flag. Usage (bash): `ARNIS_ELEV_ZOOM=13 arnis --output-dir ./world --bbox <bbox> --terrain` (PowerShell: `$env:ARNIS_ELEV_ZOOM='13'`).
- **`--master-origin-lat` / `--master-origin-lng` (tile mode).** Anchors the projection and the elevation/land-cover grid to one global lat/lng so every cell shares a single Minecraft XZ ruler and lines up block for block; also flips Arnis into "tile mode" (skips the per-run stale-tile cache cleanup and the global outlier-filter pass) and fixes the roughly 0.1% avg-lat haversine stretch. Both must be passed together. Usage: `arnis --output-dir ./world --bbox <cell-bbox> --master-origin-lat 52.5200 --master-origin-lng 13.4050`.
- **`--elevation-min` / `--elevation-max` (global band).** Pins one shared real-world min/max in metres so every tile maps height to Y identically, instead of each tile picking its own range and meeting neighbours at a vertical staircase. Both required together; set alongside `--master-origin`. Usage: `arnis ... --master-origin-lat <lat> --master-origin-lng <lng> --elevation-min 0 --elevation-max 1200`.
- **Automatic flat low-scale bridges.** At `--scale 0.3` or lower, every bridge becomes a flat 1-block deck that hugs the terrain (Beam style forced, no arch), because at 1:3 or smaller a rising arch with columns and clearance collapses into noise and overshoots the tiny span. Not a flag; triggered purely by `--scale`. Usage (implicit): `arnis --output-dir ./world --bbox <bbox> --scale 0.1`.
- **Big water / shore / wetland system plus 9 new block IDs.** A large multi-pass water, underwater, shore and wetland system (flat per-component water surface, single-cell SAND shore swap, depth-tiered bed palette, water carving under bridges, stoney-shore ring, thin-land drown) that resolves the long-running stepped-water, double-slope, AIR-hole and stray-sand artifacts. Adds 9 new block IDs (256 to 265) and widens `Block::id()` from u8 to u16. Automatic when `--terrain` is on; shore and water noise respond to the `--seed` value, with no dedicated flag.
- **Snow-capped terrain above the real-world snow line.** Terrain above the latitude-derived climatic snow line gets a thin snow layer (a new `SNOW_LAYER` block at id 266), with a 6-block noise jitter so the edge is not a hard contour. It honours the `--elevation-min/--elevation-max` lock so the cap lands at the same Y on both sides of a tile seam, and it skips water and shore (gated on the same water-blend isoline the shore uses). Automatic when `--terrain` is on.
- **Parking lots rendered like roads.** Parking areas use a speckled asphalt mix (honouring `surface=*`) with white `WHITE_CONCRETE` space markings and a slim metal lamp post, instead of flat gray concrete. The lamp post is gated behind buildings, so `--no-buildings` draws only the flat pavement, no posts.

### Changed / Engine

- **Merged 53 upstream louis-e/arnis commits.** Brings in-process tile parallelization, stream-to-disk region eviction, the mimalloc allocator, the large-area warning, and the GUI ETA, while keeping the cross-tile seam intact (0 of 1024 chunks differ in verification). The `transformation.rs` Local plus master-origin path is the seam crux.
- **Product renamed to "Arnis Meld Fork".**
- **GUI footer now credits louis-e and Teddy563.**
- **Upstream sync.** Ported the genuinely-new commits from the upstream 2.9.0 line that compose with
  the fork: the progress-bar sheen animation, a clearer extend-build-height tooltip, leisure=marina
  maps to water, a corrected u16 block-id note, the snow line, the parking visual upgrade, and the
  one-byte section storage split (see Added and Performance). The CI was also brought green (rustfmt
  plus a dead-code allow on the unused Web Mercator path). Upstream changes that fight the fork's
  design were deliberately skipped: road-width-by-lanes (our `--road-detail` already caps lanes in
  compact mode), and the upstream water beds (the fork's water rewrite is already a superset).

### Performance

- **One byte per cell for the common section.** Widening `Block` to u16 had doubled every full
  section's backing vector (a roughly 70% peak-memory jump on a dense city). Section storage now uses
  `Full(Vec<u8>)` for the overwhelmingly common case where every block id fits in a byte, and
  `FullWide(Vec<Block>)` only for the rare sections that hold an id of 256 or more. Reads, writes,
  iteration and the seam-merge content hash are representation-independent (a section hashes
  identically either way), verified by a round-trip plus a Full-versus-FullWide canary test.

### Fixed

- **Parallel-safe terrain tile fetch.** Under Meld's roughly 64 concurrent fetches, cells were getting rate-limited and truncated tiles that became flat seams. Fetch now uses 6 retries with exponential backoff, a deterministic per-tile jitter, an atomic temp-then-rename cache write, and retry-missing rounds, killing the staircase seam at the build center.
- **i32 corner-sum overflow panic on far-from-origin master coordinates.**
- **Cross-tile coordinate drift under master-origin.** `transform_point` derived metres-per-degree-longitude from `avg_lat`, shearing the grid like a fan so cells slid off the shared 512-block region grid by an amount that grows with distance from the origin, leaving flat-grass strips at tile seams; now anchored at `origin_lat` so a longitude maps to one block-X project-wide. Plain single-world generation is byte-identical.
- **Tree trunk apex cap.**
- **Minecraft 1.21.4 to 26.1.x datapack schema overlays.**
- **GUI disk-probe false-block fixes.**

## [1.8.4] - 2026-06-09

### Fixed
- **Cross-tile coordinate drift under `--master-origin-lat/lng`.** `transform_point` derived metres-per-degree-longitude from `avg_lat = (point_lat + origin_lat) / 2`, so the **same longitude mapped to a different block-X depending on the point's latitude** - the grid sheared like a fan instead of a rectangle. Invisible in a single world, but for external schedulers that stitch adjacent bboxes into one Minecraft world (Meld) it pushed each tile off the shared 512-block region grid by an amount that **grows with distance from the origin**, leaving flat-grass **"unrendered" strips at tile seams** that widen toward the far corners. Now anchored at `origin_lat` so a longitude maps to **one** block-X project-wide, matching the latitude axis (which already uses the constant `METERS_PER_DEG_LAT`). Plain single-world generation (no master origin) is byte-identical. Adds a latitude-invariance regression test.

## [1.8.3] - 2026-06-02

### Added
- 9 new Block IDs (256-265, u16 widening): `MAGMA_BLOCK`, `SUGAR_CANE`, `KELP`, `TALL_SEAGRASS_BOTTOM/TOP`, `SEA_PICKLE`, `BROWN_CANDLE_{2,3,4}`, `SOUL_SAND`, each with correct NBT side-table arms.
- Per-cell underwater bed picker with domain-warped noise - organic vanilla-MC-style patches (CLAY/SAND/DIRT/COARSE_DIRT) on a GRAVEL background.
- Rare 5-13 cell MAGMA + SOUL_SAND vents at depth ≥ 5 (bubble columns in MC).
- Underwater dunes: width-aware amplitude 2-4 blocks, domain-warped, prominent waves.
- SEAGRASS meadow mix (short + tall + sea pickle); KELP min-3-cell variable-height columns.
- Wetland post-pass: MOSS_BLOCK ring + COARSE_DIRT 2-ring around water puddles.
- Tiered cattail: 1-2 stalks + single `candles=1/2/3/4` BROWN_CANDLE block.
- Shore-land rare cattail (2%) + sugar_cane (1%) on SAND/DIRT/COARSE/GRASS at water-edge.
- `sweep_floating_veg` post-pass: removes cattail/grass/candles/sugar_cane/flowers over water cells + roads; trees (LOG/LEAVES) explicitly excluded.
- `--seed` (alias of `--tile-invariant-rendering`) now drives global noise seed → identical seed reproduces identical bed/dune/shore patterns.
- STONE under-bed fill 12 cells below `bed_y-1` → no AIR pockets in neighbour columns.

### Changed
- Slope curve: linear → `depth = local_max × √(dist/span)` (rounded, steeper near shore).
- Shore palette: high-frequency coord-hash bins → smooth `value_noise` (scale 16) static fade SAND → COARSE_DIRT → GRASS_BLOCK over 5-cell band.
- Stoney shore variant (STONE_BRICKS / COBBLESTONE / etc neighbour): COBBLE 50% + MOSSY_COBBLE 30% + COARSE_DIRT 20%.
- Underwater bed `under_block` always STONE (was SANDSTONE under SAND surface).
- Wetland branch matches `arnis-source-water` per-subtype structure (drops earlier 8-bin roll); less grass overuse, more bare MUD.

### Fixed
- **Fauna carving holes in bed**: `set_block_absolute(veg, .., None, Some(&[]))` empty-list whitelist was *always-replace* (matched nothing → fell through). Now uses `Some(&[AIR])` so veg only paints into AIR cells.
- **Veg placed inside dunes** (visual holes): vegetation now planted at `bed_top + 1` where `bed_top = bed_y + dune_bump_at(x,z)` instead of `bed_y + 1`.
- **Bed surface AIR pockets visible through translucent water**: STONE under-fill.
- **Floating veg over flooded wetland cells + roads**: `sweep_floating_veg` post-pass.
- **5×5 bridge ring** in OSM water-polygon carve (matches LC_WATER pass) → clean GRAVEL bed under causeway shadows, no DIRT strips at diagonals.

### Versioning
- Bumped to **1.8.3** (Meld fork numbering, downstream of Teddy563/arnis 2.8.1 and louis-e/arnis v2.8.0 base).

### Internal
- `value_noise_01` reads from `OnceLock<NOISE_SEED>` set by `ground_generation::set_noise_seed`, called once at the top of `generate_world_with_options`.
- New helpers: `dune_bump_at`, `sweep_floating_veg`.
- `Block::id()` return type widened `u8` → `u16` (required for IDs 256-265).

---

## [2.8.1] - 2026-05-21 (Teddy563)

Previous Meld release. Voxelize / 3DMR work + Meld scheduler + tile-invariant rendering + road-detail palette improvements.

## [v2.8.0] - 2026-05-19 (louis-e upstream)

Base upstream release inherited by Meld v1.8.3. Adds the BigWaterField depth carve + initial wetland G3 + universal LC_WATER carve pass.
