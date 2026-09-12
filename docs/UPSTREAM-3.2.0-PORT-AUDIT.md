# Porting upstream arnis 3.2.0 into the Teddy563 fork

Measured on 2026-09-12 against `louis-e/arnis@338a1d28` (tag `v3.2.0`) and the
fork branch `wip/quality-combined` (3.1.8, `78215bdb`).

## The numbers

| | |
|---|---|
| Merge base | `c7b5f19d` ("Merge pull request #1294 from louis-e/fix-datapack-building-height") |
| Upstream commits since base | **133** (543 files, +71 815 / -1 840) |
| Fork commits since base | **297** |
| `git merge-tree` conflicts, fork x upstream | **76 files** (67 content, 9 modify/delete) |
| Upstream commits with **zero** fork file overlap | **22** |
| Fork build today | `cargo check --all-targets` clean |
| Meld build today | 542 tests pass |

A straight `git merge upstream/main` is not the right shape. The 76 conflicted
files include every file the fork rewrote hardest - `buildings.rs` (fork +6 171
lines, upstream +2 204), `highways.rs`, `world_editor/*`, `overture.rs`,
`args.rs` - so one merge would be a single unreviewable blob. Cherry-pick by
subsystem instead, each bucket on its own branch, merged into
`release/3.2.0-upstream-merge` as it goes green.

## What upstream actually added

| # | Bucket | Commits | Lines | New files | Conflict risk | Verdict |
|---|---|---|---|---|---|---|
| 1 | Correctness fixes | ~12 | ~900 | 0 | **low** | take first |
| 2 | Performance | ~7 | ~1 500 | 0 | low-med | take second |
| 3 | Assets + locales | ~9 | ~3 000 | 2 | **none** | take, mechanical |
| 4 | Voxy LOD pregeneration | 4 | 2 040 | 4 | low | take third |
| 5 | Overture vector tiles | 8 | 5 488 | 4 | **high** | manual port |
| 6 | Moon / Mars (celestial) | ~8 | ~1 500 | 2 | med-high | take fifth |
| 7 | Mapillary facade pipeline | 19 | **41 423** | 35 | med | defer or last |
| 8 | Preset building facades | 5 | 4 211 | 4 | med | needs 7 first |

### 1. Correctness fixes - the cheap win

Worth having regardless of whether the fork ever takes a facade. Small diffs,
real bugs, mostly in files the fork touched only lightly.

- `8334643d` Fix crash-prone range and overflow paths
- `3449a22f` Guard cache walks, level.dat writes and invented node ids
- `e9d322c8` Key entity dedup on uuid so two entities can share a cell
- `bea583c5` Keep the node ids of ring vertices the world edge did not cut
- `9ed5b198` Fix invalid biome data
- `c8e50edc` Fix trees placed on sealed surface
- `a8b4518b` Remove magma blocks from water body floors
- `90869002` Fix extended build height with elevation terrain
- `3851573b` Remove GUI setup panic path
- `da50d99e` Place door steps only where the run reaches the ground
- `ff5705df` Minor building fixes
- `33ee2d8c` Fix reported time delta to use generation-only time when available

Fork-specific caveats:

- `90869002` touches 22 files the fork also changed, and the fork owns
  `--height-headroom` / `--height-underroom` / `--min-y` / `--max-y`, which do
  not exist upstream. Port by hand, do not cherry-pick.
- `c8e50edc` and `a8b4518b` land in `ground.rs` / `water_depth.rs`, both of
  which the fork rewrote (`river_bed.rs`, `water_depth.rs` +946). Expect a
  hand-merge; verify against the fork's river-bed golden output.

### 2. Performance

- `f19148d0` Speed up large-area generation
- `287c594b` Use per-section block palettes
- `11b5c6fe` / `63aea9f3` Restore dense Earth section storage
- `f6210e46` Minor RAM memory improvement
- `dedba696` Size the tile fetch pool to the machine (Overture-only, see #5)

`287c594b` (per-section palettes) overlaps the fork's `--region-format blinear`
work in `world_editor/java.rs`. Take it only after the fork's region-format
tests are green, and re-run them after.

### 3. Assets, locales, chrome - no conflict, mechanical

- `0224475f` better plane schems + jetbridge (`src/structures/jetbridge.rs`)
- `0694c24e` refreshed Wikidata 3D model index
- `17b0e2f2` concrete powder colors in map preview palette
- `95d7677a` / `4f1e4fb6` / `a180a8b8` Georgian + missing locales
- `79fc1628` blank map picker on networks that block the tile hosts
- `bbaa7111` / `9e98e8f0` Linux EGL zink fallback retry
- `cfcc2d75` license credits, `1862163e` press assets URL

Locale JSONs conflict textually (the fork added its own keys) but resolve by
union: take upstream's new keys, keep the fork's.

### 4. Voxy LOD pregeneration - `--voxy-lod`

`901468b4`, `3923a47b`, `4a36cc50`, plus `src/voxy/{mod,lod,mapper,rocks}.rs`.
Almost entirely a new directory plus one arg and a call site. Implies
`--bake-lighting`, which the fork already ships and Meld already drives. Good
value per line and a clean Meld toggle.

### 5. Overture vector tiles - the one that genuinely fights the fork

Upstream split `src/overture.rs` into
`src/overture/{mod,cache,mvt,pmtiles,tiles}.rs` and added
`--overture-source {auto,tiles,parquet}`. The fork has **1 028 lines of its own
changes in `src/overture.rs`**, including the prewarm path
(`--prewarm-overture`) that Meld depends on.

Do not cherry-pick this. Port it as:

1. Take upstream's `src/overture/` directory wholesale onto a branch.
2. Re-apply the fork's `overture.rs` deltas into `overture/mod.rs` by hand, one
   hunk at a time.
3. Confirm `--prewarm-overture` still works end to end from Meld before merging.

`acf8de43` ("Harden the tile reader against malformed and hostile archives") and
`5b91e97d` ("Stop a directory from sizing an allocation by its own claim") are
input-validation fixes on a network-fed archive parser. If the tiles transport
is taken at all, these two are not optional.

### 6. Moon / Mars

`2b3905e1` and follow-ups add `src/celestial.rs`,
`src/elevation/providers/planetary.rs`, and `--body {earth,moon,mars}`.
`apply_body_defaults()` force-sets scale, mode, overture, 3d, interior, trees,
height limit and world time. That function is the integration point: the fork
adds its own options (`--caves`, `--snow-mode`, `--rocks`, `--bushes`,
`--field-mix`, `--props`, ...) that also have to be cleared for an airless body,
or a Moon world tries to plant bushes.

`2b3905e1` touches 37 files the fork also changed. Second-hardest bucket after
the facades.

### 7-8. Mapillary + preset facades - 45 634 lines

The largest thing upstream shipped, and mostly **additive**: 35 new files under
`src/mapillary/`, 4 under `src/building_facades/`, plus golden fixtures under
`tests/golden/facade/`. The conflict surface is small (hooks in `buildings.rs`,
`world_editor/common.rs`, `args.rs`, the GUI) relative to the volume.

New CLI surface, all of it gated behind a Mapillary token:

```
--mapillary-token <TOKEN>          (env MAPILLARY_TOKEN, hide_env_values)
--mapillary-facades [true|false]   default: on once a token exists
--mapillary-probe                  sample coverage and exit
--mapillary-debug-dir <DIR>
--mapillary-facades-dir <DIR>
--mapillary-facade-debug-dir <DIR>
--mapillary-facade-debug-walls <k1,k2>
--mapillary-facade-mode {blocks,photos}
--building-facades                 preset facades, no token needed
--building-facades-dir <DIR>
--facade-detail {standard,high}
--facade-px {4,8,16,32}
```

Recommendation: **do not put this in fork 3.2.0.** It is 63% of upstream's whole
delta, it needs a third-party API token to exercise at all, and
`--mapillary-facade-mode photos` places item display entities that only work on
Java 1.21.4+, which collides with the fork's `--mc-version` handling. Ship it as
fork 3.3.0 once buckets 1-6 are green and the fork's own regression hashes are
stable.

If it is wanted in 3.2.0 anyway, take it as one squashed "vendor upstream
mapillary subsystem" commit rather than 19 cherry-picks - the intermediate
states do not build against fork code.

## Fork-side surface that constrains every bucket

The fork carries 60+ CLI options upstream does not have. The ones that collide:

| Fork option | Collides with |
|---|---|
| `--overture` prewarm path | bucket 5, the `overture.rs` -> `overture/` split |
| `--region-format blinear`, `--blinear-level` | `287c594b` per-section palettes |
| `--mc-version` | facade item display entities (Java 1.21.4+ only) |
| `--min-y` / `--max-y` / `--height-headroom` / `--height-underroom` | `90869002` extended build height |
| `--caves`, `--rocks`, `--bushes`, `--props`, `--field-mix`, `--snow-mode` | `apply_body_defaults()` for Moon/Mars |
| `--gpu`, `--void-world`, `--level-name` | `e8a5a795` upstream custom world name |

`e8a5a795` ("Add optional custom world name for Java worlds") is a **duplicate
feature**: the fork already has `--level-name`. Do not port it, or the GUI ends
up with two name fields.

`591c2232` removes upstream's Paintings facade mode - irrelevant unless bucket 7
is taken.

## Recommended order

```
release/3.2.0-upstream-merge          <- integration branch (created)
  port/3.2.0-fixes                    bucket 1 + 2   ~19 commits
  port/3.2.0-assets                   bucket 3       ~9 commits, mechanical
  port/3.2.0-voxy                     bucket 4       4 commits + new dir
  port/3.2.0-overture-tiles           bucket 5       manual, hand-merged
  port/3.2.0-celestial                bucket 6       ~8 commits
  (port/3.3.0-facades                 buckets 7 + 8, deferred)
```

Gate between each: `cargo check --all-targets`, `cargo test`, the fork's own
block-hash regression, and one real generation over a known bbox compared
against a 3.1.8 render of the same bbox.

## Honest confidence

| Bucket | Coverage achievable | Confidence | Why |
|---|---|---|---|
| 1 Fixes | ~95% | high | small diffs, reviewable, testable |
| 2 Perf | ~85% | med-high | `287c594b` interacts with blinear regions |
| 3 Assets | 100% | high | mechanical |
| 4 Voxy | ~95% | high | additive |
| 5 Overture | ~80% | **medium** | hand-merge of 1 028 fork lines; the fallback path is where a mistake hides |
| 6 Celestial | ~85% | medium | `apply_body_defaults` must learn 15 fork options |
| 7-8 Facades | ~90% if taken | **low-medium** | 45k lines, needs a live API token to exercise; the golden fixtures are upstream's, not the fork's |

Weighted by lines and **excluding** facades, buckets 1-6 reach roughly 88-92%
coverage at high-to-medium confidence. **Including** facades, overall confidence
drops to medium at best, because two thirds of the delta would be code nobody
has run against fork geometry.

The 90% target is reachable for 3.2.0 if 3.2.0 is defined as buckets 1-6. It is
not reachable in one pass if facades are in scope.

## Meld 1.9.9

Meld drives arnis entirely through `src/arnis_cmd.py`, and every flag there is
already gated by `arnis_supports(exe, "--flag")`, which greps the binary's
`--help`. That is the whole compatibility mechanism: one Meld build talks to
both 3.1.8 and 3.2.0-fork, and a flag the binary does not advertise is simply
not emitted. No version sniffing, no branching on a version string.

So 1.9.9 needs, and only needs:

1. New settings keys plus `arnis_supports`-gated emission for whichever upstream
   flags land in the fork.
2. Matching toggles in the existing settings UI.
3. Tests asserting each new flag is emitted when supported and absent when not.

That layer can be written and tested **before** the arnis merge lands: against a
3.1.8 binary every new flag probes False and Meld's output is byte-identical to
1.9.8, which is exactly the regression guarantee wanted.
