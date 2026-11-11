# 23. Benchmark: Arnis 3.4.0 against pieces, on one machine

Written 2026-11-11 from `docs/bench/results.jsonl` (session 1, task B2) and `docs/bench/results-b3.jsonl` (session 2, task B3). Every number below is computed from those two files by `docs/bench/make_md.py`. The one-page comparison is `docs/23-ARNIS-VS-CONFLUENCE.pdf`.

## What this says

- Building in pieces trades a little speed for much lower RAM. It is not a speed-up.
- Stock 3.4.0 is faster at 64 km² (15.3 s against 16.3 s) and at 576 km² (94.4 s against 103.1 s). Of the three areas, tuned Auto is faster only at 256 km² (52.8 s against 57.9 s), where it also uses 53.1% less RAM.
- Tuned Auto matches the best sweep result only at 256 km².
- Peak RAM drops from 7.9 GB to 4.8 GB at 576 km², at the cost of 9.2% more wall time.

## Setup

Machine: 24 threads, 31 GB RAM, Windows 11, one machine. All areas are squares around Bucharest (44.43 N, 26.10 E), built at the default scale. Data caches were warm unless a row says cold. Each warm arm ran 3 times, interleaved with its partner arm so a slow minute hits both.

| Area | Bounding box (S,W,N,E) | km² | Regions |
|---|---|---|---|
| S8 | `44.3909,26.0522,44.4627,26.1528` | 64 | 256 |
| S16 | `44.3549,26.0019,44.4987,26.2031` | 256 | 1024 |
| S24 | `44.3190,25.9515,44.5346,26.2535` | 576 | 2209 |

Builds: stock is `arnis-v3.4.0.exe` (sha256 `aafa66e9...`). Session 1 used the Confluence.4 branch tip, commit a78668b6 (`arnis-tip.exe`, sha256 `571f629f...`). Session 2 used the tuned build, branch `c35/c4-tune`, commit 1cefd515 (`arnis-tuned.exe`, sha256 `b295e96b...`; the break-even W runs used `arnis-tuned-be.exe`, `b0318c8c...`, whose fixed-count path is the same). `cpu_s` is the last per-second sample of the process tree's CPU time, so it under-counts runs of a few seconds. Failed, timed-out or out-of-memory runs: 0.

**The two sessions differ.** Session 2 ran about 20% faster than session 1 on the same stock exe (S16: 72.7 s, then 57.9 s). Machine state varies. So the tuned numbers are compared with stock reruns from session 2 (`A0b3`), never with session 1.

## Results per area

### S8, 64 km²

| Arm | Runs | Wall s, min / median / max | Peak RAM MB | CPU s (median) | World MB | Result |
|---|---|---|---|---|---|---|
| A0 stock 3.4.0, session 1 | 3 | 15.6 / 18.4 / 19.4 | 2658 | 163 | 984 | exit 0 in 3 of 3 |
| A0b3 stock 3.4.0, session 2 | 3 | 15.2 / 15.3 / 15.3 | 2580 | 156 | 984 | exit 0 in 3 of 3 |
| A2 old Auto, tip build, session 1 | 3 | 29.6 / 29.6 / 31.6 | 1729 | 163 | 1030 | exit 0 in 3 of 3 |
| A2b3 tuned Auto (one run), session 2 | 3 | 16.3 / 16.3 / 17.3 | 2590 | 168 | 996 | exit 0 in 3 of 3 |

### S16, 256 km²

| Arm | Runs | Wall s, min / median / max | Peak RAM MB | CPU s (median) | World MB | Result |
|---|---|---|---|---|---|---|
| A0 stock 3.4.0, session 1 | 3 | 63.1 / 72.7 / 77.3 | 7238 | 671 | 3924 | exit 0 in 3 of 3 |
| A0b3 stock 3.4.0, session 2 | 3 | 57.8 / 57.9 / 58.9 | 7334 | 642 | 3924 | exit 0 in 3 of 3 |
| A1 tip build, no flags (regression check) | 1 | 75.5 / 75.5 / 75.5 | 7284 | 665 | 3924 | exit 0 in 1 of 1 |
| A2 old Auto, tip build, session 1 | 3 | 88.2 / 93.0 / 95.1 | 2830 | 574 | 4053 | exit 0 in 3 of 3 |
| A2b3 tuned Auto, 16 pieces of 8x8, 4 workers x 5 threads | 3 | 51.1 / 52.8 / 52.8 | 3438 | 622 | 4019 | exit 0 in 3 of 3 |
| W fixed 8x8 pieces, 4 workers, tuned build, session 2 | 3 | 51.8 / 52.8 / 52.8 | 3345 | 617 | 4019 | exit 0 in 3 of 3 |

### S24, 576 km²

| Arm | Runs | Wall s, min / median / max | Peak RAM MB | CPU s (median) | World MB | Result |
|---|---|---|---|---|---|---|
| A0 stock 3.4.0, session 1 | 3 | 99.8 / 116.8 / 120.5 | 8147 | 1205 | 8813 | exit 0 in 3 of 3 |
| A0b3 stock 3.4.0, session 2 | 3 | 94.3 / 94.4 / 95.4 | 8059 | 1169 | 8813 | exit 0 in 3 of 3 |
| A2 old Auto, tip build, session 1 | 3 | 161.3 / 169.3 / 212.7 | 3566 | 1229 | 9020 | exit 0 in 3 of 3 |
| A2b3 tuned Auto, 36 pieces of 8x8, 4 workers x 5 threads | 3 | 103.0 / 103.1 / 103.3 | 4915 | 1408 | 8983 | exit 0 in 3 of 3 |

The world from pieces is a little larger than the stock world (+2.4% at S16); the cause was not investigated.

## Sweep: piece size and worker count (S16, session 1, tip build)

Fixed settings `--one-world --unit-regions N --one-world-workers W`, 3 runs per cell, interleaved. Cell = median s (min-max) and peak RAM.

| Regions per piece side \ workers | 2 | 4 | 6 |
|---|---|---|---|
| 2x2 | 140.3 (137.9-141.1), 739 MB | 101.9 (100.8-103.0), 1076 MB | 94.8 (94.7-146.3), 1401 MB |
| 4x4 | 83.3 (82.4-85.6), 1382 MB | 68.2 (66.6-71.3), 2116 MB | 71.0 (70.1-72.2), 2707 MB |
| 8x8 | 71.1 (67.8-72.1), 2611 MB | **64.9 (64.8-67.2), 3298 MB Recommended** | 72.0 (69.7-74.0), 4399 MB |

Rule (user ruling 2026-11-10): the winner is the lowest median wall time whose peak RAM stays under 70% of RAM (22221 MB; every cell does). If the runner-up's median falls inside the winner's min-max range, it is a tie and the cell with the lower peak RAM wins.

Result: lowest median 8x4 (64.9 s, range 64.8-67.2); runner-up 4x4 at 68.2 s is outside that range, so this is a clear win, not a tie. **Recommended: 8 regions per side, 4 workers.** Cells are regions per side x workers. The smallest cell (2x2 pieces, 2 workers) takes 2.2 times as long as the winner but peaks at 739 MB instead of 3298 MB.

## Break-even: where pieces start to win (session 2, interleaved, 3 runs each)

Stock against the fixed winner cell (8x8 pieces, 4 workers). Squares share S16's centre.

| Area | km² | Stock median (min-max) s | Pieces median (min-max) s | Pieces against stock | Peak RAM, stock / pieces MB |
|---|---|---|---|---|---|
| B100 | 100 | 23.4 (23.3-24.8) | 28.5 (28.4-28.8) | +21.8% | 3261 / 3018 |
| B150 | 150 | 35.6 (34.6-36.6) | 36.5 (36.5-36.6) | +2.5% | 4533 / 3197 |
| B200 | 200 | 45.7 (44.6-45.8) | 42.6 (42.6-42.7) | -6.8% | 5941 / 3220 |
| S16 | 256 | 57.9 (57.8-58.9) | 52.8 (51.8-52.8) | -8.8% | 7334 / 3345 |

Pieces are slower at 100 km², level at 150 km², and ahead by 3% or more from 200 km² (first row at or below -3%). Above that, only S16 and S24 were measured: 256 km² is a win for pieces, 576 km² is a loss (+9.2% for tuned Auto). Nothing between 256 and 576 km² or above 576 km² was measured, so the upper edge of the pieces' advantage is unknown. That is the open question in the limits below.

## Tuned Auto against stock (session 2)

| Area | Stock median s | Old Auto median s (session 1) | Tuned Auto median s | Tuned against stock | Peak RAM, stock / tuned MB |
|---|---|---|---|---|---|
| S8 (64 km²) | 15.3 | 29.6 | 16.3 | +6.5% | 2580 / 2590 |
| S16 (256 km²) | 57.9 | 93.0 | 52.8 | -8.8% | 7334 / 3438 |
| S24 (576 km²) | 94.4 | 169.3 | 103.1 | +9.2% | 8059 / 4915 |

Tuned Auto against the fixed winner cell on S16, same session: 52.8 s against 52.8 s (+0.0%). Against the sweep's 64.9 s it reads -18.6%, but that gap is the difference between sessions, not the tune. At S8 tuned Auto now builds in one plain run (below 220 km²), so its +6.5% against stock is not explained by a measurement.

## What changed in `budget.rs`

From the tune branch `c35/c4-tune` (commits 780f9445 and 1cefd515) against 7310c28a. Line numbers are in `src/scale/budget.rs` unless stated.

| Constant or rule | Before (7310c28a) | After (c35/c4-tune) |
|---|---|---|
| `MIN_THREADS` | 4 (line 21) | 5 (line 22) |
| `AUTO_CPU_TARGET` | 75 (line 25) | 90 (line 26) |
| `MIN_WORKERS` / `MAX_WORKERS` | 2 / 6 (lines 22, 23) | 2 / 6 (lines 23, 24), unchanged |
| `estimate_piece_mb`, MB per region at scale 0.5 or more | 19 (line 31); an 8x8 piece is 1616 MB | 9 (line 34); an 8x8 piece is 976 MB, 4x4 is 544 MB |
| `AUTO_SINGLE_BELOW_KM2` | none | 220 km² (line 63): Auto with no other job flag builds smaller selections in one plain run |
| `WORKER_S_PER_KCHUNK` (`src/work_units.rs`) | [0.68, 1.98] (line 594) | [0.18, 0.28] (line 609) |
| Auto cut (`src/work_units.rs`) | mixed pieces of 2 to 8 regions, smallest first | whole 8x8 pieces clipped at the edges |

Measured piece memory on S16: an 8x8 piece peaked at 825 MB per worker (estimate 976, +18%), a 4x4 at 529 MB (estimate 544, +3%). With more threads per piece the peak grows (8x8 with 10 threads: 1305 MB); `Sizing::cap_for` lowers the worker count once a piece reports. A later commit on that branch, 1e93bd7c, tells the window when Auto builds a single run; it changes no timing and was not benchmarked.

The window's time estimate for a piece job uses the middle of `WORKER_S_PER_KCHUNK`, divided by the workers. On S16 that gave 57.5 s against a measured 52.8 s (+9%). The constants come from one dense city, so rural jobs will read low. `arnis-tuned.exe` was built with the old `WORKER_S_PER_KCHUNK`; only the plan line's estimate differs from the committed code.

## Cold caches

Cold arms use an empty `ARNIS_CACHE_ROOT` and one run each, on S16, session 1, tip build, with the old Auto. Stock 3.4.0 ignores `ARNIS_CACHE_ROOT`, so there is no cold stock run.

| Arm | Wall s | Peak RAM MB | CPU s | World MB |
|---|---|---|---|---|
| A2 old Auto, warm (3 runs, median) | 93.0 | 2830 | 574 | 4053 |
| A2cold old Auto, cold | 144.9 | 2242 | 582 | 4053 |
| A4cold old Auto with `--prewarm-first`, cold | 264.7 | 2846 | 742 | 4053 |

A cold start took 55.8% more wall time than a warm one. Warming the caches first (`--prewarm-first`) made the cold run 82.7% slower, because it runs pieces one at a time to fill the cache. It stays off by default.

## Rerun it

The drivers sit next to the test exes, in `arnis-integration/work/bench22/` of the Meld workspace (they are not part of the Arnis repository). Run them from that folder.

```
python bench.py --arm NAME --bbox S,W,N,E --exe PATH [--cache warm|cold] --repeat N --cap-min M --flags FLAG...   # one arm, appends to results.jsonl
python run_b2.py            # session 1: warm-up, A0, A1, A2, the 27-run sweep, cold arms (resumable)
python run_b3.py be         # break-even: stock against the fixed winner on 100, 150, 200 km2
python run_b3.py auto       # tuned Auto on S8, S16, S24
python run_b3.py ctl        # same-session S16 control (fixed winner and stock)
python run_b3.py ctl2       # same-session stock on S8 and S24
python analyze.py           # winner, tie rule, break-even, tuned Auto against stock
```

This folder rebuilds the tables and the PDF from the two jsonl files: `python docs/bench/make_md.py` and `python docs/bench/make_pdf.py` (needs Microsoft Edge for the PDF; no other dependency).

## Limits

- One machine (24 threads, 31 GB), one dense city, three areas plus three break-even squares. A rural area, a slower disk or less RAM can move every number here.
- Three runs per warm arm; the cold arms have one run each. Medians of three are coarse; the min-max is shown for that reason.
- The two sessions differ by about 20%. Only same-session pairs are compared.
- Stock is faster at 64 and 576 km². Tuned Auto does not switch pieces off above some area, because no upper edge was measured. Open question: measure about 400 km² and about 1024 km² (a world of roughly 16 GB), then set an upper threshold. At 576 km² on this machine the stock build is faster.
- Below 220 km² the window's plan still draws pieces and a piece time estimate, although the run is a single run.
- The time estimate was fitted on one dense city.
- `--prewarm-first` was measured cold, once.
