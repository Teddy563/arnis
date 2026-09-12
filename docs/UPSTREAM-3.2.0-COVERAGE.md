# How much of upstream 3.2.0 this fork carries

Written for: whoever decides what goes in the release announcement, and whoever picks up the
3.3.0 port. Measured against `louis-e/arnis@338a1d28` (tag v3.2.0) from the merge base
`c7b5f19d`: **133 commits, 104 of them non-merge, +71 815 / −1 840 lines over 543 files.**

There is no single honest percentage, because the three ways of counting disagree by a
factor of three. All three are below.

## By commit — 39% of what applies

| | commits | share of the 104 |
|---|---|---|
| Ported | 23 | 22% |
| Fork already has its own equivalent | 8 | 8% |
| Deferred (facades, Overture tiles, jet bridge) | 48 | 46% |
| Does not apply (upstream's own GUI, CI, version bumps, locale plumbing) | 25 | 24% |

Of the 79 commits that could apply here, **31 are covered — 39%**. Counting the 25 that owe
this fork nothing, 54% of all 104 are accounted for.

The number is dominated by one thing: 28 of the 48 deferred commits are the facade feature,
built in many small steps.

## By line — 24% of upstream's delta is even portable

| | insertions | share |
|---|---|---|
| Facades (Mapillary + presets, incl. the bundled image pack) | 51 668 | 72% |
| Overture vector-tile transport | 3 231 | 4% |
| Everything else | 16 916 | 24% |

Of that last 24%, this fork carries substantially all of it.

## By feature — 85% of what a user would name

| Upstream 3.2.0 feature | Here |
|---|---|
| Moon and Mars (`--body`) | ✅ ported |
| Voxy LOD pregeneration (`--voxy-lod`) | ✅ ported |
| 18 new blocks (+ Luanti and Bedrock maps) | ✅ ported, renumbered to 450–467 |
| Georgian localisation, refreshed Wikidata index | ✅ ported |
| Magma off water floors | ✅ ported |
| Bedrock biome padding 0 → 0xFF | ✅ ported |
| Quarry ore-roll overflow | ✅ ported |
| Entity dedup keyed on UUID | ✅ ported |
| Iron door upper half on Bedrock | ✅ ported |
| Trees and vegetation off sealed surfaces | ✅ ported |
| Concrete powder in the map-preview palette | ✅ ported |
| Overture roof strings interned | ✅ ported |
| Per-section palettes, dense storage, large-area speedups | ➖ fork has its own equivalent |
| Custom world name | ➖ fork already has `--level-name` |
| **Preset building facades** | ❌ deferred |
| **Mapillary street-photo facades** | ❌ deferred |
| **Overture vector-tile transport** | ❌ deferred |
| **Jet bridge prop** | ❌ deferred |

**14 of 18, or 85%** — 78% if the two facade sources are counted separately.

## Why the deferred four are deferred

- **Facades (both sources).** Upstream rebuilt its building wall renderer around a
  `FacadePlan`: the window lattice moves from `bx + bz` to an ordinate measured along each
  wall, walls are classified party/street/corner, buildings take a category. This fork
  rebuilt the same renderer around its architectural-era and window-frame grammar, and its
  interiors, loot and signage anchors are written against that. They are alternatives, not
  layers. The first attempt to merge them produced 594 compile errors, 239 in
  `buildings.rs` alone.
- **Overture vector tiles.** Upstream split `overture.rs` into a module directory; this fork
  has 1 028 lines of its own in that file, including the prewarm path Meld drives. A
  hand-merge, not a port.
- **Jet bridge.** Needs a free-yaw schematic routine this fork does not have.

## Getting to 90%+

Only one thing moves the number materially: the facade work. That is 51 668 lines against a
renderer this fork rewrote for other reasons, so it is a rewrite of this fork's
`buildings.rs` against upstream's plan model — not a port, and not a weekend. Everything
else upstream shipped in 3.2.0 is already here.

## Commands, for whoever picks this up

The deferred work is reachable by path, not by merge:

```
git checkout upstream/main -- src/mapillary src/element_processing/building_facade.rs
git checkout upstream/main -- src/overture            # the module directory
git diff c7b5f19d upstream/main -- src/element_processing/buildings.rs
```
