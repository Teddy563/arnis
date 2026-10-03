# Advanced Features

Settings > **Advanced Features** in the GUI shows controls for performance
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
| Piece Size | `--unit-regions` | One World only. 2 to 8 regions per side, default 4. |

With One World on, Parallel Workers and Piece Size always build the area in
pieces (see [Large areas](one_world.md#large-areas)).

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
