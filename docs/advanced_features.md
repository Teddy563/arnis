# Advanced Features

Settings > **Advanced Features** in the GUI shows a few controls for
performance and large worlds. With the switch off, or a field on Auto / 0,
Arnis runs exactly as it does without them. Each control is a CLI flag, and
the CLI has a few more for scripts and programs driving it. None of them
changes what is generated.

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
