# Extra Features

Settings > **Extra Features** in the GUI (named Advanced Features before;
the control ids and stored settings keep that name) shows controls for performance
and large worlds, and the experimental Meld Generation options. With the
switch off, or a field on Auto / its default, Arnis runs exactly as it does
without them. Each control is a CLI flag, and the CLI has a few more for
scripts and programs driving it. The performance controls never change what is
generated; the Meld Generation ones do.

## GUI

| Control | CLI flag | Notes |
| --- | --- | --- |
| CPU Usage | `--cpu-target` | Share of the cores, 10 to 100%. |
| Threads | `--threads` | Exact thread count; wins over CPU Usage. |
| Memory Budget | `--ram-budget-mb` | 0 reads free memory as usual. |
| Parallel Downloads | `--max-downloads` | Default 16. |
| Big Worlds | | On with the switch. Splits every selection into cells built in parallel; see below. |
| Parallel Workers | `--one-world-workers` | Needs Big Worlds. Auto or 1 to 6. |
| Cell Size | `--unit-regions` | Needs Big Worlds. 2x2, 4x4 or 8x8 regions per piece, default 4x4. A stored 3, 5, 6 or 7 from an older version falls back to 4x4. |
| Selection Snap | | Needs Big Worlds. Fit Inside (default) or Cover; see below. |
| Square Selection | | Needs Big Worlds. The same number of cells both ways. |

**Big Worlds** turns on with the switch, and can be turned off, which gives
the usual single run and no grid. With it on, a selection is built in pieces (see
[Large areas](one_world.md#large-areas)) whenever it can be: with One World on,
always; with it off, when the snapped selection is more than one cell, and the
run then makes a new One World named as a new world is ("Arnis World N"),
pinned with `--origin` to the snap. The readout under the selection says
"Builds as a One World". A selection of one cell, or one on Bedrock, Luanti or
another body than Earth, is the usual single run of what was drawn ("Builds in
one run"). Pieces need a Java world on Earth.

The size line under the selection follows the same rule. Built in pieces, it
reads "Builds in N pieces · W workers", W being Parallel Workers or what Auto
picks for this machine. Otherwise a large selection gets the usual size
warning, and with the switch off the warning carries a link, "Use Extra
Features to build it in pieces", that opens this section and turns the switch
on (Big Worlds too).

With Big Worlds on (on Earth), the map selection snaps to whole cells (one cell
is Cell Size regions, one piece) on the world's lattice, which is anchored at
block (0, 0): an existing One World's own, or for a new one the frame its
first area will create. **Selection Snap** picks how:

- **Fit Inside** (default) keeps the whole cells that lie inside the
  selection, so nothing outside what was drawn is built. On a new world the
  cells are centred on the selection: block (0, 0) is its centre when both
  counts are even, and moves half a cell on a side whose count is odd, so the
  centre is the middle of a cell (still a region junction, as cells are 2, 4
  or 8 regions). On an existing world the lattice is fixed, so the cells are
  the ones inside. A side with no whole cell inside gets one, around the
  selection's centre, and the readout says so.
- **Cover** grows the selection outward to whole cells. On a new world block
  (0, 0), the corner of four regions and of four cells, is the centre of the
  selection, so the snap has the same number of cells either side.

**Square Selection** makes both sides the same number of cells, centred: the
smaller count with Fit Inside, the larger with Cover.

The map draws the snapped outline and the block (0, 0) dot always, and the cell
lines only once a cell is at least 8 pixels on screen (and the snap has at most
2,000 cells), so a country-sized selection zoomed out draws three shapes. The
overlay is redrawn when the zoom ends or the selection changes, never while
panning. Cell lines are yellow dashes on a dark halo, the outline solid
yellow, so both read on light and dark tiles. **Show grid**, a button under
the map's world toggle shown while Big Worlds is on, hides or shows the overlay (on by default, remembered); the
snap and the run do not change. The readout under the selection gives width (east-west) by height
(north-south): `12 × 8 regions · 3 × 2 cells · 6 pieces · 6.1 × 4.1 km`, the
kilometres being blocks over the world scale. Generation is given the snapped
bbox; the frame maths is the run's own (`work_units::snap_to_cells`). A new
world is also given `--origin` at the snap's block (0, 0), so its frame does
not depend on the bbox and every edge, however tall the selection, lands on a
cell line.

`--origin LAT,LON` (CLI, `--one-world` only) sets block (0, 0) of the world a
run creates, instead of the centre of its first bbox. A world that already
exists keeps the origin it was created with; the run says so and goes on.

### Meld Generation (Experimental)

A control on its default, or greyed out, sends no flag.

| Group | Control | CLI flag | Default | Description |
| --- | --- | --- | --- | --- |
| Terrain | Snow | `--snow-mode` | Realistic | Realistic (climatic snow line), Peaks, Manual or Off. |
| Terrain | Snow Cap Share | `--snow-percent` | 6% | Peaks only: top share of the height range under snow. Greyed with One World, which refuses Peaks. |
| Terrain | Snow Line Y | `--snow-y` | 120 | Manual only: snow from this Y up. |
| Terrain | Rocks | `--rocks` | Off | Small rocks on open grass and cropland. |
| Terrain | Bushes | `--bushes` | Off | Small bushes on open grass and cropland. |
| Terrain | Rock Density | `--rock-density` | 0.02 | Share of chunks that get a rock (0 to 0.20). Needs Rocks. |
| Terrain | Bush Density | `--bush-density` | 0.05 | Share of chunks that get a bush (0 to 0.20). Needs Bushes. |
| Roads & Buildings | Road Detail | `--road-detail` | Max | Max, Clean (from about 0.7 scale) or Compact (lower scales). |
| Roads & Buildings | No Buildings | `--no-buildings` | Off | Roads, rail, water and land cover only. |
| Roads & Buildings | Chest Loot Table | `--loot-table` | Built-in | JSON loot file for interior chests. Needs Interior Generation. A file that does not load stops the run. |
| Fields & Trees | Field Layout | `--field-mix` | Classic | Classic, Smallholding, Patchwork, Prairie or Pasture parcels. |
| Fields & Trees | Farm Crops | `--farm-crops` | Empty | Crop shares, e.g. `wheat=60,sunflower=20,fallow=20`. |
| Fields & Trees | Parcel Size | `--field-scale` | 100% | 25 to 400% of the layout's parcel size. Needs a layout or farm crops. |
| Fields & Trees | Tree Realm | `--tree-realm` | Auto | Force one region's trees (Africa, Asia, Europe, ...). |
| Fields & Trees | Small / Medium / Big / Tall / Giant Trees | `--tree-size-weights` | 100% each | 0 to 200% per size; sent only when one differs from 100%. |
| Caves & Water | Cave Seed | `--cave-seed` | Empty | Another cave layout per seed. Needs Caves. |
| Caves & Water | Cave Datum Y | `--cave-datum-y` | Empty | Floor Y for the caves, a multiple of 16, so areas line up. Needs Caves. |
| Caves & Water | River Bed | `--river-bed` | Off | U-Shaped (`v1`) beds for rivers, canals and streams. |
| Caves & Water | Water Detail | `--water-detail` | Default | Scaled: finer water for small scales. |

The window checks these with the CLI's own parser and its snow and cave-datum
rules, and passes them on to every piece of a One World job.

### Experimental (Phase 3)

The last group under Meld Generation. As above, a control on its default, or
greyed out or hidden, sends no flag.

| Control | CLI flag | Default | Description |
| --- | --- | --- | --- |
| Region Format | `--region-format` | Anvil | Anvil (`.mca`) or B_Linear (`.b_linear`, Leaf 1.21.11+ servers only). Java only, greyed with One World. |
| B_Linear Level | `--blinear-level` | 6 | zstd level 1 to 22. Shown for B_Linear only. |
| Climate Sampling | `--climate-mode` | Origin | Origin or Per Position. |
| Climate Map: Preview | `--climate-map` | | Draws the climate zones of the selected area in the window. |
| Grass Texture | `--grass-texture` | Off | Mapped meadows as loose parcels. |
| Land Texture | `--land-texture` | Off | Untagged land textured from satellite land cover. |
| Grass Mix | `--grass-mix` | Empty | Preset or share list. Needs Grass or Land Texture. |
| Land Mix | `--land-mix` | Empty (patchwork) | Preset or share list. Needs Land Texture. |
| World Floor / World Ceiling | `--min-y` / `--max-y` | Empty (auto) | Needs Extend Build Height on a Java world; greyed with One World. |
| World Seed | `--seed` | Empty | Another, repeatable look per seed. |
| Props | `--props` | Auto | Auto follows 3D Models (no flag); All, None, or Custom with a family checklist. |
| Props Minimum Scale | `--props-min-scale` | Empty | No props below this world scale. |
| Tree Pack Folder | `--tree-pack-dir` | Empty (`tree-packs` next to Arnis) | Your own tree schematics, see [Tree Pack Folder](#tree-pack-folder). Sent only when the folder exists; the status line under the rows reads "*n* custom trees found (*m* skipped)". |
| Tree Pack Mode | `--tree-pack-mode` | Add to Built-in | Add to Built-in or Replace Built-in. Sent with the folder. |
| Tree Pack Layout: Create Folder Structure / Export Built-in Trees | `--init-tree-pack-dir` / `--export-tree-packs` | | Run in the window on the Tree Pack Folder. |
| World Border | `--world-border` | Off | Sets the world border around the generated area after the run. Java only. |
| Redraw One World Map: Redraw | `--map-item-only` | | Repaints the One World's map item over every area. Needs One World. |

The window applies what the CLI does for these: the seed, the tall
datapack's floor and ceiling (checked as the CLI checks them) and the region
format. The two buttons run in the window, not as a generation.

### Option Previews

Under Field Layout, Tree Realm, the tree size sliders, Snow, Rocks, Bushes,
Road Detail, River Bed, Climate Sampling, Grass Texture and Land Texture a
256x160 card shows the selected option, from
`src/gui/images/previews/<setting>-<value>.png`. Each was built by the
release CLI over a small sample area and cut from its `--map-preview` PNG
(`arnis --bbox=AREA --output-dir=DIR --map-preview --no-3d FLAG`, where FLAG
is the option, e.g. `--field-mix=prairie`), then quantized to 128 colours:

| Cards | Sample area (`--bbox`) | Notes |
| --- | --- | --- |
| Field Layout, Rocks, Bushes | `44.55,26.00,44.555,26.008` | Rocks and Bushes on `--field-mix=pasture` (rocks avoid tilled farmland), density 0.2. |
| Tree Realm, tree sizes | `44.2000,25.9000,44.2050,25.9080` | Tree sizes: one run per size with only it weighted, cut into five strips. |
| Snow | `46.53,7.95,46.54,7.965` | Manual with `--snow-y=180`. |
| Road Detail | `44.4450,26.0950,44.4470,26.0980` | |
| River Bed | `44.4300,26.0850,44.4340,26.0950` | Water shaded by depth from the region files: the bed cannot be seen from above. |
| Grass Texture | `46.6200,8.0400,46.6240,8.0460` | |
| Land Texture | `44.6000,25.7000,44.6050,25.7080` | |
| Climate Sampling | `25.0,-5.0,65.0,45.0` | The `--climate-map` PNG; Origin is the centre's climate everywhere. |

Snow, Rocks & Bushes, Road Detail, Field Layout and the trees (Tree Realm
with Tree Sizes) show their controls as a list beside one 4:3 picture of the
combination, captioned with it (e.g. "Europe · Tall + Giant"). Tree Sizes is a
switch per size over its weight (off is 0, on from 0 is 100). The picture
zooms (wheel, pinch, +/−) and pans by dragging; a double-click fits it again.
A 2D | 3D switch over it, kept per viewer, picks the top-down card or an
isometric one from `src/gui/images/previews/iso/<card>.webp`; where no 3D
picture exists yet (live renders included) the 2D one shows with "3D preview
coming". The 3D pictures are 1280x960, drawn by `iso_render.py` beside
`make_previews.py`.

The script that does all of this (`make_previews.py`, Python with Pillow and
nbtlib) is kept with the Meld tooling, not in this repository; the table and
the command above are enough to redo any card by hand.

The Tree Realm card also renders live with a Tree Pack Folder: its flags
belong to the trees group, and the render cache key includes the folder's
file list (path, size, time), so editing the folder gives a new card.

**Live previews.** When a group's other settings leave the stock defaults
(Farm Crops or Parcel Size for the fields, a tree size weight, Snow Cap
Share or Snow Line Y, a density, a Grass or Land Mix, Water Detail; any
Rocks or Bushes setting), the window asks `gui_render_preview` for that
group, 600 ms after the last change. It builds a card-sized piece of the
group's sample area (the row's area above, at the card's zoom, centred where
the shipped card was cut) with this executable as a CLI run, with only the
group's flags (plus `--field-mix=pasture` for Rocks and Bushes) and stock
defaults for everything else, and draws it the same way. The card shows the
shipped picture until the render arrives, dimmed and captioned "Updating".
Renders take about one to one and a half seconds once the sample's data is
cached, and are kept under the cache root in `arnis/option-previews`, keyed
by group, flags and Arnis version, so a combination seen before is instant.
With Offline Mode on the render reads the caches only; if they lack the
sample area the card keeps the shipped picture and says "Preview needs data".

## OSM Data Source

Its own settings section, after Extra Features. It is not behind the
Extra Features switch: these flags go with every run, and the defaults are
the stock downloads.

| Control | CLI flag | Default | Description |
| --- | --- | --- | --- |
| Download Plan: Download & Bake What's Missing | `--prewarm` | | Shown with a selection while Offline Mode is on or the source is Region Download, Local File or Local Archive. Per source (OpenStreetMap, elevation, land cover, canopy height, Overture): Cached, Partly cached (n/m) or Missing, with an estimated download; for Region Download, the Geofabrik extract the cached index picks and whether a bake holds the area; for Local Archive, the archives in the folder that cover the selection and the Prepare Countries list (see [Local Archive](#local-archive)). Read from disk only; the button runs the download and checks again. |
| Source | | Arnis Tile Archive | Arnis Tile Archive (no flag), Overpass (`--no-tile-archive`), Local File (`--file`), Region Download (Geofabrik) (`--osm-pbf`) or Local Archive (Baked Countries) (`--osm-tiles-url <folder>`). |
| Archive URL | `--osm-tiles-url` | Empty (Arnis's archive) | Shown for the tile archive. |
| Archive Folder | `--osm-tiles-url` | Empty (`<cache>/arnis/local-archive`) | The local archive folder. Shown for Local Archive. |
| arnis-tiles Path | | Empty | Where arnis-tiles is when it is not next to Arnis or on PATH: the program or its folder. Shown for Local Archive. |
| Overpass Servers | `--overpass-url` | Empty (Arnis's server) | Comma list, tried in order. Shown for Overpass. |
| Local File | `--file` | Empty | An `.osm`, `.xml` or Arnis `.json` file; the area is still the map selection. Shown for Local File. |
| PBF File | `--osm-pbf` | Empty (`geofabrik`) | An `.osm.pbf` extract; empty sends `--osm-pbf=geofabrik`. Shown for Region Download. |
| Bake Selection: Bake Now | `--prewarm` | | The download button's run, offered beside the extract: downloads it if needed and bakes the selection. Shown for Region Download. |
| Offline Mode | `--offline` | Off | Caches only. A run that misses something stops and lists what. |
| Download Area For Offline Use: Download | `--prewarm` | | Downloads what a generation of the selected area would read, with the current settings, and builds nothing. Progress on the main bar. Runs as a child CLI process. |
| Warm Caches Before Building Pieces | `--prewarm-first` | Off | Needs a One World built in pieces (Extra Features on); greyed with Offline Mode. |

All of them reach the pieces of a One World job too.

## Local Archive

A local tile archive is what the Arnis tile archive is, baked on this computer
from whole Geofabrik country extracts by
[arnis-tiles](https://github.com/louis-e/arnis-tiles): the same AOT1 format,
zoom 13 tiles and decoder, so a world built from it is the same as one built
from the public archive of the same data date. `--osm-tiles-url` takes the
folder (or a `file://` URL) and reads `archives.json` and the `.pmtiles` files
in place: no HTTP, no cache copy, and it works with `--offline`. A country is
baked once and serves every selection inside it.

In the window, pick **Local Archive (Baked Countries)** as the Source. The
Download Plan then shows:

- which archives in the folder hold the selection, read from `archives.json`
  and its coverage cells (z6) on disk. OpenStreetMap shows Cached when every
  z13 tile of the selection falls in an archive's cells, so Offline Mode with
  a fully covered selection reads Cached ✓;
- **Prepare Countries**: the extracts `arnis-tiles prepare --dry-run` picks for
  the selection (the cheapest Geofabrik extracts covering it), with their
  download size and Baked ✓ for those already in the folder. The dry run runs
  in the background when the selection changes and is kept per bbox for the
  session;
- **Download & Bake** runs `arnis-tiles prepare` for the selection into the
  folder, with the Extra Features Threads or CPU Usage (else 75 % of the
  cores). Progress goes on the main bar: the download share, then
  "Baking ..." with the elapsed time (the bake reports nothing until it ends),
  then writing the archive. **Stop** ends it; countries already baked are
  kept, and arnis-tiles resumes the rest next time. The plan is checked again
  when it ends.

arnis-tiles keeps its Geofabrik index and size cache in
`<cache>/arnis/arnis-tiles/`, and its bake state in `<folder>/work`.

Sizes and cost, Romania (2026-10): a 330 MB `.pbf` download, a 457 MB archive
(23,455 tiles), about 1.6 GB of RAM while baking. arnis-tiles stops a bake when
less than 15 GB is free on the folder's disk. Bucharest from this archive and
from the public archive gives the same world.

The same from a terminal:

```sh
arnis-tiles --cache <cache>/arnis/arnis-tiles/cache --out <folder> prepare --bbox 44.40,26.00,44.50,26.20 --dry-run
arnis-tiles --cache <cache>/arnis/arnis-tiles/cache --out <folder> prepare --bbox 44.40,26.00,44.50,26.20 --threads 12
arnis --bbox 44.445,26.095,44.448,26.103 --osm-tiles-url <folder> --offline --output-dir <saves>
```

**Finding arnis-tiles.** Arnis looks next to its own executable (where a
release bundle puts it), then on PATH, then at **arnis-tiles Path**. Without
it the panel says where to get it and Download & Bake stays off. A release
bundles it as a Tauri sidecar:

```sh
# in a clone of louis-e/arnis-tiles
cargo build --release
# in arnis: the sidecar name carries the target triple
mkdir -p binaries
cp <arnis-tiles>/target/release/arnis-tiles.exe binaries/arnis-tiles-x86_64-pc-windows-msvc.exe
cargo tauri build --config tauri.sidecar.conf.json
```

`tauri.sidecar.conf.json` adds `bundle.externalBin: ["binaries/arnis-tiles"]`.
It is a separate file because Tauri's build script fails when a listed
external binary is missing, which would break every plain `cargo build`.

## Tree Pack Folder

`--tree-pack-dir FOLDER` (experimental) loads your own tree schematics beside
the bundled packs, laid out the way the bundled packs are organised:

```
FOLDER/<realm>/<community>/<tree type>/[<size>/]<any name>.schem
```

- **realm**: `afr`, `asn`, `aus`, `ena`, `eur`, `fl`, `ind`, `sam`, `wna` or
  `vanilla-plus` (the pack `--tree-realm` names). The area's location picks
  the realm as usual.
- **community**: one of the realm's forest types (the names in its
  `region.json`, e.g. `EUR - Alpine forest (mature)`); case does not matter.
  An unknown community is skipped: the ecoregion tree mixes pick communities
  by name, so a new one would never be chosen.
- **tree type**: the species. Any folder name; a new one adds a species to the
  community, and its part before the first `_` is the genus (palm and conifer
  checks go by genus, as for the bundled trees).
- **size**: optional `small` (up to 6 blocks tall), `medium` (7-12), `big`
  (13-20), `tall` (21-28) or `giant` (29 and up), the thresholds of
  `tree_library::size_for_height`. Outside a size folder a file gets the size
  its height gives. `--max-tree-size`, `--tree-size-weights` and the
  giant-only-at-1:1 rule apply to it like to any tree.

File names are free; `.schem` and `.schematic` (Sponge v2/v3) are read, other
files (`README.txt`, ...) are ignored. Every file goes through the pack's
`.schem` loader when the run starts; one that does not parse, has no leaves,
is over 8 MB or over 256 blocks on a side, or sits outside the layout is
skipped with one `Warning: tree-pack-dir: skipped ...` line. Files are taken
in path order (`/`-separated, relative to the folder), so two machines with
the same folder place the same trees. User trees count as narrow trunks (the
bundled `w1` class).

`--tree-pack-mode add|replace` (default `add`):

- `add`: the folder's trees join the bundled ones of their community.
- `replace`: in every realm the folder has at least one usable tree for, the
  bundled trees are dropped and only the folder's are used; communities left
  empty drop out of that realm. Realms without user trees keep the bundled
  set. Communities that ecoregion mixes load from other packs come from the
  folder too.

`--init-tree-pack-dir FOLDER` creates the empty layout of every bundled pack
(10 realms, 172 communities, 1056 tree types, each with the five size
folders) and a `README.txt` at the root and in each realm, then exits.
`--export-tree-packs FOLDER` does the same and writes every bundled schematic
(3746 files) into the size folder its height gives, to edit or adapt. An
exported folder in `replace` mode holds the bundled trees again (eur: 355 of
355), but not the same picks: the files come in folder order and the
wide-trunk variants lose their rarity. In `add` mode it doubles them.

With the flags absent nothing changes: the pack reads the compiled-in
manifests and files as before. The folder rides on every piece's command line
of a One World job, and picks stay a function of the block position, so
pieces still meet without a seam. The capability name is `tree-pack-dir`
(covering all four flags).

In the window: Extra Features > Experimental, rows Tree Pack Folder, Tree
Pack Mode and Tree Pack Layout (see the table above).

## Presets

**Save Preset** and **Load Preset**, under the Extra Features switch (and
shown whether it is on or off), write and read every Extra Features and OSM
Data Source setting, the switch included, as a JSON file
(`{"arnisPreset": 1, "settings": {...}}`, keyed by control id). Loading sets
every setting it covers; one the file leaves out, or holds a value it cannot
take, goes back to its default.

## B_Linear Container

`--region-format blinear` writes the same file Meld's `region-convert`
(`--to blinear-v3`) writes from the same world saved as `.mca`, byte for byte,
at the same level. Checked on a Paris test area, void (399 chunks) and flat
(1024 chunks), levels 6 and 19: our `.b_linear` taken to `.mca` and back with
`region-convert` comes out identical, `region-convert --info` reports no
warnings or discarded chunks, and both decoders read every chunk of the other's
files. Pinned by `region_convert_writes_the_same_bytes`.

Field by field (`RC` = `region-convert/src`, ours = `src/world_editor/blinear.rs`):

| Field | Ours | region-convert | Value |
| --- | --- | --- | --- |
| Superblock | `blinear.rs:33` | `RC/formats/mod.rs:21` | i64 `-0x2008_1225_0269`, big-endian |
| Version | `blinear.rs:34`, `:101` | `RC/formats/blinear_v3.rs:289` | u8 `3` |
| Level byte | `blinear.rs:102` | `RC/formats/blinear_v3.rs:290` | u8, the zstd level used |
| Level range | `blinear.rs:63`, `args.rs` (1..=22, default 6) | `RC/cli.rs:147`, `RC/formats/mod.rs:112` (1..=22, default 6) | same |
| Hash seed | `blinear.rs:35`, `:103` | `RC/formats/mod.rs:23`, `RC/formats/blinear_v3.rs:291` | u32 `0x0721` |
| Bucket table | `blinear.rs:111` | `RC/formats/blinear_v3.rs:345` | 16 x u64 absolute offsets from byte 14 |
| Bucket layout | `blinear.rs:37`, `:87` | `RC/formats/blinear_v3.rs:18-20`, `:298` | 64 slots, index `x + z * 32`, ascending |
| Empty bucket | `blinear.rs:107`, `:126` | `RC/formats/blinear_v3.rs:316` | no record, offset 0 |
| Bucket record | `blinear.rs:112-113` | `RC/formats/blinear_v3.rs:334-336` | i32 raw length, i32 compressed length, frame |
| Compression | `blinear.rs:151` | `RC/formats/blinear_v3.rs:321` | `zstd::bulk::compress`, zstd 0.13.3 / libzstd 1.5.7 |
| Absent slot | `blinear.rs:136` | `RC/formats/blinear_v3.rs:312` | i32 `0` |
| Slot length | `blinear.rs:143` | `RC/formats/blinear_v3.rs:306-308` | i32 `nbtLen + 16` |
| Section | `blinear.rs:144-148` | `RC/formats/mod.rs:380-390` | i32 nbtLen, i64 timestamp, u32 xxh32(nbt, seed), nbt |
| Timestamp | `blinear.rs:159-165` | `RC/formats/mca.rs:59`, `RC/formats/mod.rs:393` | `.mca` header seconds x 1000: 0 void, flat template otherwise |
| Footer | none | none | none |
| File name | `blinear.rs:65` | `RC/formats/mod.rs:102-106` | `r.X.Z.b_linear` |
| Publish | `blinear.rs:78` (temp + rename) | `RC/writer.rs:89`, `:131` (temp + rename) | same |

Arnis's chunk NBT itself is not byte-stable between runs (compound key order
changes), in `.mca` as well; two runs compare equal as NBT, not as bytes.

## Flags

- `--threads N`: worker threads for generation. Default 90% of the cores, or
  `RAYON_NUM_THREADS` when set; this flag wins over both. Conflicts with
  `--cpu-target`.
- `--cpu-target P`: the same worked out as a share of the cores (10-100).
- `--ram-budget-mb MB`: memory the run may assume, used in place of the free-RAM
  reading when deciding whether to stream regions to disk and how many the
  flush queue holds, so several processes can split one machine.
- `--max-downloads N`: downloads kept in flight at once (default 16): elevation,
  Mapillary, 3D models and the Overture fetch pool.
- `--no-update-check` (or `ARNIS_NO_UPDATE_CHECK=1`): skip the check for a newer
  release, so a script starting many runs does not make it once per process.
- `--no-cache-sweep` (or `ARNIS_NO_CACHE_SWEEP=1`): skip the startup sweep of
  cached files older than 30 days, for a cache managed or kept offline
  elsewhere.
- `ARNIS_CACHE_ROOT=<dir>`: every cache lives under this folder instead of the
  OS cache folder, so several processes can share one warm cache on a drive of
  your choosing.
- `--progress json`: also print progress as JSON lines on stdout,
  `{"v":1,"type":"phase"|"progress"|"error"|"done",...}`; `done` carries
  `wall_s`, `cpu_s`, `peak_rss_mb` and `chunks`. A job in pieces adds `piece`
  records. Other output is unchanged; readers keep the lines starting `{"v":`.
  The format is described in `src/progress_json.rs`.
- `--timeout S`: flood-fill budget in seconds, counted as work rather than wall
  time (one second is 200 million point-in-polygon edge tests), so a fill is
  cut at the same place however loaded the machine is. The GUI uses 40.
- `--plan-units N`: print how `--one-world` would cut `--bbox` into pieces of at
  most N x N regions, as one JSON line, and exit.
- `--unit-regions N`: build a `--one-world` selection in pieces of at most
  N x N regions (1-64). An interrupted job resumes when run again.
- `--one-world-workers auto|N`: build that many pieces at once, each in its own
  process (1-64, or `auto` for 1 to 6 from the cores and free memory).
- `--osm-pbf PATH|geofabrik`: read OpenStreetMap from an `.osm.pbf` extract
  instead of the tile archive, with no fallback. `geofabrik` picks the smallest
  Geofabrik region whose border polygon holds the whole selection (a 5x5 grid
  of points, ranked by polygon area; the index is cached for a week) and
  downloads it once to `<cache>/arnis/osm-pbf/downloads`. The selection plus
  the One World clip pad (64 blocks) is cut out the way the Overpass query
  would (every way whose extent meets the area, wanted relations with all
  their member ways, the ways' nodes and the wanted tagged nodes) and kept as
  a bake under `<cache>/arnis/osm-pbf/bakes/<extract>/`, keyed by the
  extract's name, size and time and by the area. Any bake holding the area is
  reused, so a repeat run or a smaller area inside it does not read the
  extract. The extract is decoded on the run's rayon pool, so `--threads`,
  `--cpu-target` and a piece's thread share apply. Reading keeps every node
  location in memory, 16 bytes a node (Romania, 330 MB and 41 million nodes,
  peaks at 1.0 to 1.6 GB). With `--unit-regions` the coordinator bakes the
  job once before the pieces start and each piece cuts its area from that
  bake. Works with `--offline` once the extract is downloaded. Conflicts with
  `--file`.
- `--osm-pbf-url URL`: the extract `--osm-pbf geofabrik` downloads, instead of
  the one it would pick.
- `--world-border`: Java only. After the run, writes the world border into
  `level.dat` (`Data.BorderCenterX`/`Z` at the centre of the generated area in
  blocks, `BorderSize` and `BorderSizeLerpTarget` its longer side, so the
  shorter side has room to spare), leaving damage and warning distances as
  they were. For a One World the area is the bounds of every area the
  manifest holds; a job in pieces sets it once, in the coordinator, after the
  last piece.
- `--capabilities`: print the feature names this build supports as one JSON
  array (`["progress-json","threads",...]`) and exit, so a program can probe
  the binary before using them. Names are only ever added.
