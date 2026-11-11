"""Writes docs/23-BENCH-PIECES-VS-SINGLE.md. Every benchmark number is computed from results*.jsonl
(this folder) through make_pdf.py's helpers. Usage: python make_md.py"""
from make_pdf import AREAS, BAD, G, HERE, breakeven, gb, pct, st, sweep

s, win, run, tie, pick = sweep()
OUT = HERE.parent / "23-BENCH-PIECES-VS-SINGLE.md"


def n_ok(arm, area):
    return f"{len(G[(arm, area)])}/{len(G[(arm, area)])} ok"


def row(label, arm, area):
    x = st(arm, area)
    return (f"| {label} | {x['n']} | {x['min']:.1f} / {x['med']:.1f} / {x['max']:.1f} | {x['rss']} | "
            f"{x['cpu']:.0f} | {x['world']:.0f} | exit 0 in {x['n']} of {x['n']} |")


HEAD = ("| Arm | Runs | Wall s, min / median / max | Peak RAM MB | CPU s (median) | World MB | Result |\n"
        "|---|---|---|---|---|---|---|")
ARMS = {
    "S8": [("A0 stock 3.4.0, session 1", "A0"), ("A0b3 stock 3.4.0, session 2", "A0b3"),
           ("A2 old Auto, tip build, session 1", "A2"), ("A2b3 tuned Auto (one run), session 2", "A2b3")],
    "S16": [("A0 stock 3.4.0, session 1", "A0"), ("A0b3 stock 3.4.0, session 2", "A0b3"),
            ("A1 tip build, no flags (regression check)", "A1"),
            ("A2 old Auto, tip build, session 1", "A2"), ("A2b3 tuned Auto, 16 pieces of 8x8, 4 workers x 5 threads", "A2b3"),
            ("W fixed 8x8 pieces, 4 workers, tuned build, session 2", "W")],
    "S24": [("A0 stock 3.4.0, session 1", "A0"), ("A0b3 stock 3.4.0, session 2", "A0b3"),
            ("A2 old Auto, tip build, session 1", "A2"), ("A2b3 tuned Auto, 36 pieces of 8x8, 4 workers x 5 threads", "A2b3")],
}
bbox = {a: G[("A0", a)][0]["bbox"] for a in AREAS}
reg = {a: G[("A0", a)][0]["regions"] for a in AREAS}

parts = [f"""# 23. Benchmark: Arnis 3.4.0 against pieces, on one machine

Written 2026-11-11 from `docs/bench/results.jsonl` (session 1, task B2) and `docs/bench/results-b3.jsonl` (session 2, task B3). Every number below is computed from those two files by `docs/bench/make_md.py`. The one-page comparison is `docs/23-ARNIS-VS-CONFLUENCE.pdf`.

## What this says

- Building in pieces trades a little speed for much lower RAM. It is not a speed-up.
- Stock 3.4.0 is faster at {st('A0b3','S8')['km2']:.0f} km² ({st('A0b3','S8')['med']:.1f} s against {st('A2b3','S8')['med']:.1f} s) and at {st('A0b3','S24')['km2']:.0f} km² ({st('A0b3','S24')['med']:.1f} s against {st('A2b3','S24')['med']:.1f} s). Of the three areas, tuned Auto is faster only at {st('A0b3','S16')['km2']:.0f} km² ({st('A2b3','S16')['med']:.1f} s against {st('A0b3','S16')['med']:.1f} s), where it also uses {pct(st('A2b3','S16')['rss'], st('A0b3','S16')['rss'])[1:]} less RAM.
- Tuned Auto matches the best sweep result only at {st('A0b3','S16')['km2']:.0f} km².
- Peak RAM drops from {gb(st('A0b3','S24')['rss'])} to {gb(st('A2b3','S24')['rss'])} at {st('A0b3','S24')['km2']:.0f} km², at the cost of {pct(st('A2b3','S24')['med'], st('A0b3','S24')['med'])[1:]} more wall time.

## Setup

Machine: 24 threads, 31 GB RAM, Windows 11, one machine. All areas are squares around Bucharest (44.43 N, 26.10 E), built at the default scale. Data caches were warm unless a row says cold. Each warm arm ran 3 times, interleaved with its partner arm so a slow minute hits both.

| Area | Bounding box (S,W,N,E) | km² | Regions |
|---|---|---|---|
""" + "\n".join(f"| {a} | `{bbox[a]}` | {st('A0', a)['km2']:.0f} | {reg[a]} |" for a in AREAS) + f"""

Builds: stock is `arnis-v3.4.0.exe` (sha256 `aafa66e9...`). Session 1 used the Confluence.4 branch tip, commit a78668b6 (`arnis-tip.exe`, sha256 `571f629f...`). Session 2 used the tuned build, branch `c35/c4-tune`, commit 1cefd515 (`arnis-tuned.exe`, sha256 `b295e96b...`; the break-even W runs used `arnis-tuned-be.exe`, `b0318c8c...`, whose fixed-count path is the same). `cpu_s` is the last per-second sample of the process tree's CPU time, so it under-counts runs of a few seconds. Failed, timed-out or out-of-memory runs: {len(BAD)}.

**The two sessions differ.** Session 2 ran about 20% faster than session 1 on the same stock exe (S16: {st('A0','S16')['med']:.1f} s, then {st('A0b3','S16')['med']:.1f} s). Machine state varies. So the tuned numbers are compared with stock reruns from session 2 (`A0b3`), never with session 1.

## Results per area
"""]
for a in AREAS:
    parts.append(f"### {a}, {st('A0', a)['km2']:.0f} km²\n\n{HEAD}\n" + "\n".join(row(l, arm, a) for l, arm in ARMS[a]) + "\n")
a0, a2 = st("A0b3", "S16"), st("A2b3", "S16")
parts.append(f"""The world from pieces is a little larger than the stock world ({pct(a2['world'], a0['world'])} at S16); the cause was not investigated.

## Sweep: piece size and worker count (S16, session 1, tip build)

Fixed settings `--one-world --unit-regions N --one-world-workers W`, 3 runs per cell, interleaved. Cell = median s (min-max) and peak RAM.

| Regions per piece side \\ workers | 2 | 4 | 6 |
|---|---|---|---|
""" + "\n".join(
    f"| {u}x{u} | " + " | ".join(
        (lambda c, b: f"{'**' if b else ''}{c['med']:.1f} ({c['min']:.1f}-{c['max']:.1f}), {c['rss']} MB{' Recommended**' if b else ''}")(
            s[(u, w)], (u, w) == pick) for w in (2, 4, 6)) + " |" for u in (2, 4, 8)) + f"""

Rule (user ruling 2026-11-10): the winner is the lowest median wall time whose peak RAM stays under 70% of RAM ({0.7 * 31 * 1024:.0f} MB; every cell does). If the runner-up's median falls inside the winner's min-max range, it is a tie and the cell with the lower peak RAM wins.

Result: lowest median {win[0]}x{win[1]} ({s[win]['med']:.1f} s, range {s[win]['min']:.1f}-{s[win]['max']:.1f}); runner-up {run[0]}x{run[1]} at {s[run]['med']:.1f} s is {'inside' if tie else 'outside'} that range, so {'it is a tie and the lower RAM wins' if tie else 'this is a clear win, not a tie'}. **Recommended: {pick[0]} regions per side, {pick[1]} workers.** Cells are regions per side x workers. The smallest cell (2x2 pieces, 2 workers) takes {s[(2,2)]['med'] / s[pick]['med']:.1f} times as long as the winner but peaks at {s[(2,2)]['rss']} MB instead of {s[pick]['rss']} MB.

## Break-even: where pieces start to win (session 2, interleaved, 3 runs each)

Stock against the fixed winner cell (8x8 pieces, 4 workers). Squares share S16's centre.

| Area | km² | Stock median (min-max) s | Pieces median (min-max) s | Pieces against stock | Peak RAM, stock / pieces MB |
|---|---|---|---|---|---|
""" + "\n".join(
    f"| {n} | {k:.0f} | {x['med']:.1f} ({x['min']:.1f}-{x['max']:.1f}) | {w['med']:.1f} ({w['min']:.1f}-{w['max']:.1f}) | {pct(w['med'], x['med'])} | {x['rss']} / {w['rss']} |"
    for n, k, x, w in breakeven()) + f"""

Pieces are slower at 100 km², level at 150 km², and ahead by 3% or more from 200 km² (first row at or below -3%). Above that, only S16 and S24 were measured: 256 km² is a win for pieces, 576 km² is a loss ({pct(st('A2b3','S24')['med'], st('A0b3','S24')['med'])} for tuned Auto). Nothing between 256 and 576 km² or above 576 km² was measured, so the upper edge of the pieces' advantage is unknown. That is the open question in the limits below.

## Tuned Auto against stock (session 2)

| Area | Stock median s | Old Auto median s (session 1) | Tuned Auto median s | Tuned against stock | Peak RAM, stock / tuned MB |
|---|---|---|---|---|---|
""" + "\n".join(
    f"| {a} ({st('A0b3', a)['km2']:.0f} km²) | {st('A0b3', a)['med']:.1f} | {st('A2', a)['med']:.1f} | {st('A2b3', a)['med']:.1f} | {pct(st('A2b3', a)['med'], st('A0b3', a)['med'])} | {st('A0b3', a)['rss']} / {st('A2b3', a)['rss']} |"
    for a in AREAS) + f"""

Tuned Auto against the fixed winner cell on S16, same session: {st('A2b3','S16')['med']:.1f} s against {st('W','S16')['med']:.1f} s ({pct(st('A2b3','S16')['med'], st('W','S16')['med'])}). Against the sweep's {s[pick]['med']:.1f} s it reads {pct(st('A2b3','S16')['med'], s[pick]['med'])}, but that gap is the difference between sessions, not the tune. At S8 tuned Auto now builds in one plain run (below 220 km²), so its {pct(st('A2b3','S8')['med'], st('A0b3','S8')['med'])} against stock is not explained by a measurement.

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
""" + "\n".join(
    f"| {l} | {st(arm, 'S16')['med']:.1f} | {st(arm, 'S16')['rss']} | {st(arm, 'S16')['cpu']:.0f} | {st(arm, 'S16')['world']:.0f} |"
    for l, arm in [("A2 old Auto, warm (3 runs, median)", "A2"), ("A2cold old Auto, cold", "A2cold"),
                   ("A4cold old Auto with `--prewarm-first`, cold", "A4cold")]) + f"""

A cold start took {pct(st('A2cold','S16')['med'], st('A2','S16')['med'])[1:]} more wall time than a warm one. Warming the caches first (`--prewarm-first`) made the cold run {pct(st('A4cold','S16')['med'], st('A2cold','S16')['med'])[1:]} slower, because it runs pieces one at a time to fill the cache. It stays off by default.

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
""")
OUT.write_text("\n".join(parts), encoding="utf-8")
print(OUT)
