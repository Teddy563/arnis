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
| Parallel Workers | `--one-world-workers` | One World only. Auto or 1 to 6. |
| Cell Size | `--unit-regions` | One World only. 2x2, 4x4 or 8x8 regions per piece, default 4x4. A stored 3, 5, 6 or 7 from an older version falls back to 4x4. |

With One World on, Parallel Workers and Cell Size always build the area in
pieces (see [Large areas](one_world.md#large-areas)).

While pieces are in use, the map selection grows outward to whole cells
(one cell is Cell Size regions, one piece) on the world's lattice, which is anchored at
block (0, 0): an existing One World's own, or for a new one the frame its
first area will create, whose block (0, 0) (the corner of four regions and of
four cells) is the centre of the selection, so the snap has the same number
of cells either side. The map draws the snapped outline and the cell lines
(only the outline and the count past 2,000 cells), the readout under the
selection gives its size in regions and pieces, and generation is given the
snapped bbox. The frame maths is the run's own (`work_units::snap_to_cells`).
A new world takes the middle latitude of its first bbox for its origin, so a
selection more than about 20 km tall cannot keep both its north and south
edges on cell lines; one of them then stops a few chunks inside its line
(never outside, so no sliver of a piece row) and the readout says so.

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
| Redraw One World Map: Redraw | `--map-item-only` | | Repaints the One World's map item over every area. Needs One World. |

The window applies what the CLI does for these: the seed, the tall
datapack's floor and ceiling (checked as the CLI checks them) and the region
format. The two buttons run in the window, not as a generation.

## OSM Data Source

Its own settings section, after Extra Features. It is not behind the
Extra Features switch: these flags go with every run, and the defaults are
the stock downloads.

| Control | CLI flag | Default | Description |
| --- | --- | --- | --- |
| Source | | Arnis Tile Archive | Arnis Tile Archive (no flag), Overpass (`--no-tile-archive`) or Local File (`--file`). |
| Archive URL | `--osm-tiles-url` | Empty (Arnis's archive) | Shown for the tile archive. |
| Overpass Servers | `--overpass-url` | Empty (Arnis's server) | Comma list, tried in order. Shown for Overpass. |
| Local File | `--file` | Empty | An `.osm`, `.xml` or Arnis `.json` file; the area is still the map selection. Shown for Local File. |
| Offline Mode | `--offline` | Off | Caches only. A run that misses something stops and lists what. |
| Download Area For Offline Use: Download | `--prewarm` | | Downloads what a generation of the selected area would read, with the current settings, and builds nothing. Progress on the main bar. Runs as a child CLI process. |
| Warm Caches Before Building Pieces | `--prewarm-first` | Off | Needs a One World built in pieces (Extra Features on); greyed with Offline Mode. |

All of them reach the pieces of a One World job too.

## Presets

**Save Preset** and **Load Preset**, under the Extra Features switch (and
shown whether it is on or off), write and read every Extra Features and OSM
Data Source setting, the switch included, as a JSON file
(`{"arnisPreset": 1, "settings": {...}}`, keyed by control id). Loading sets
every setting it covers; one the file leaves out, or holds a value it cannot
take, goes back to its default.

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
- `--capabilities`: print the feature names this build supports as one JSON
  array (`["progress-json","threads",...]`) and exit, so a program can probe
  the binary before using them. Names are only ever added.
