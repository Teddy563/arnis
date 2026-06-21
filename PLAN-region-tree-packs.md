# Plan: region-aware tree packs (2271 schems, 176 communities)

Status: PLAN. No commits to main. No Co-Authored-By. Builds on the existing schematic-tree
system (`src/schematic.rs`, `src/tree_library.rs`, stamp inside `Tree::create_of_type`).

Source data (measured, extracted to `C:\tmp\treepacks`):
- 2271 `.schem` across 9 biogeographic realm packs + `vanilla-plus`. 1.6 MB total.
- ~400 unique species. Height tiers: small(<=6)=150, medium(7-12)=564, big(13-20)=720, huge(21+)=837.
- 176 `.layer` files = WorldPainter `Bo2Layer` communities. Each names a biome (e.g.
  "AFR - East African Savanna") and lists its member schems. This is the author's placement intent.
- Manifests dumped: `C:\tmp\schem_manifest.json` (per-schem dims+logs), `C:\tmp\layer_communities.json`
  (community -> schem basenames).

## Design goal

Generate trees that match the real-world location, the way a WorldPainter pro would:
realm from lat/lon -> plant community from local terrain signals -> species weighted -> size by
scale -> 4-rotation -> trunks spaced >=1 block apart. Use ALL species except the pale-garden set.

---

## Layer 0 - Realm (per generation, constant)

A single Meld selection is a few km -> one realm. Compute realm ONCE from the selection-center
lat/lon and pass `--tree-realm <code>` to every cell process. Constant => tile-invariant, zero
per-cell cost. Unknown/disabled -> `vanilla-plus`.

| code | pack | lat range | lon range |
|---|---|---|---|
| afr | african-trees | -35..37 | -18..52 |
| eur | european-mediterranean | 35..71 | -11..40 |
| asn | asian-palearctic | 20..75 | 40..150 |
| ind | indomalaya | -10..28 | 60..155 |
| ena | eastern-n-america | 8..60 | -100..-52 |
| wna | north-america-west | 25..72 | -170..-100 |
| sam | south-american | -56..13 | -82..-34 |
| aus | oceania-australia | -50..0 (+Pacific/Hawaii) | 110..180, -160 |
| fl  | florida-SE-caribbean | 8..31 | -90..-60 |
| vn+ | vanilla-plus (fallback/temperate default) | any | any |

Overlaps (Mediterranean in afr+eur, Caribbean in ena+fl+sam) resolved by priority box test
(fl before ena, eur before afr in the Med band), else nearest-centroid, else vn+. Boundaries do
not cause seams because one generation sits inside one realm.

## Layer 1 - Community (per cell, from terrain signals)

Each realm has 12-28 communities. Tag every community with habitat axes by KEYWORD on its name
(names are descriptive, so this auto-tags with high accuracy; a handful of manual overrides):

- moisture: wet {mangrove, swamp, riparian, wetland, rainforest, cypress} / dry {savanna, scrub,
  desert, xeric, thorn, mallee, caatinga, veld, steppe} / mesic {everything else}
- elevation: montane {alpine, montane, taiga, krummholz, mountain, highland, bristlecone, sequoia} / lowland
- water-adjacency: riparian/mangrove/swamp communities only fire near water

Local signals already available in Arnis per cell:
- terrain Y (DSM)        -> elevation band (montane if in top quartile of the selection's relief,
                            or above an absolute montane line)
- distance-to-water DT   -> already computed for water carving; near-water -> riparian/mangrove community
- ESA cover class        -> wetland vs forest vs shrub; already drives tree spawn
- value_noise(x,z)       -> blends between candidate communities so a realm is not monotone

Selection rule per cell (first match):
1. near water (DT <= 4) and realm has a wet community -> that community
2. ESA wetland cover -> swamp community
3. elevation montane and realm has montane community -> that community
4. else -> realm default forest (most species-rich lowland community), with value_noise
   occasionally (~10-20%) swapping to a secondary (dry/savanna in arid realms, mixed in temperate)

## LOCKED DECISIONS (2026-06-21)

- vanilla-plus is a SEASONING, not a fallback. Blend every realm: **85% regional community /
  12% vanilla-plus sprinkle / 3% rare cross-accent (an exotic from the same realm)**. Tunable.
- No-match fallback = the realm's DEFAULT FOREST community (most species-rich lowland), still
  85/12/3 blended. Never drop to plain vanilla-plus (keeps ambiguous cells location-correct).
- Region selection UI = Auto (by location) default + manual override dropdown.
- Use ALL species except the pale garden (pale-oak trees) + giant mushrooms. Tall pines included.

## Layer 2 - Species (within community)

Weighted random over the community's schems. Base weight = variant count per species (the author
shipped more files for the species they meant to be common; 1 file = accent). Optional small boost
for the community's signature species. Seed = `coord_hash(x,z)` so it is position-deterministic.

Worked example - vanilla-plus temperate default (the universal fallback, matches the earlier
"oak dominant, birch patches, dark oak rarer" spec):

| where | species mix |
|---|---|
| lowland default | oak ~55%, birch ~20%, dark oak ~8%, cherry ~5%, azalea ~2%, misc ~10% |
| near water | swamp oak (warm) / spruce-bog (cold) |
| mountains | spruce + pine (taiga), fading to oak downslope |
| dry/open (ESA) | acacia savanna |

## Layer 3 - Size (scale-aware)

Size from schem Height: small<=6, medium 7-12, big 13-20, huge 21+.

| Meld scale | sizes allowed |
|---|---|
| < 0.25 | small + medium |
| 0.25 .. 0.6 | small/medium/big; huge rare |
| > 0.6 | all; huge only where value_noise marks an "emergent" spot |

## Layer 4 - Rotation

4-way, `coord_hash(x^k1, z^k2) % 4`. Already implemented.

## Layer 5 - Spacing (the trunk-touching fix)

Problem: Arnis spawns one tree per qualifying cell -> adjacent trunks touch (see screenshots).

Fix: deterministic Poisson-disk grid filter. A tree stamps only if its (x,z) is the designated
point of its spacing cell:
- cell = (floor(x/S), floor(z/S)); designated point = cell_origin + center-biased jitter(hash(cell)).
- base S = 3 -> guarantees >= 1 empty block between ANY two trunks (center distance >= 2).
- big/huge trees thinned onto a coarser sub-grid (S=5 / S=7) so giants get more room.
- the cell's hash also picks the size class for that cell (resolves the size<->spacing circularity).
- leaves are NOT filtered -> canopies overlap freely, forest still closes. Only trunks are spaced.

Tile-invariant (pure function of x,z). Coverage still tracks Arnis (a tree appears only where Arnis
qualifies AND the cell's point is hit); dense forest -> one spaced tree per S-cell; sparse stays sparse.

---

## Block palette work (enabling step)

Schems use these blocks beyond what `map_block` currently handles:
- KEEP/REMAP (add block defs - all real MC 1.21.4 blocks):
  - `stripped_{oak,spruce,birch,jungle,acacia,dark_oak,mangrove,pale_oak}_log/wood` -> add stripped defs (fidelity) or fall back to base log
  - `pale_oak_log/wood/leaves` -> add PALE_OAK defs; used as pale trunk in baobab/eucalyptus across realms (NOT pale garden)
  - `mangrove_roots`, `muddy_mangrove_roots`, `mangrove_propagule` -> add (mangrove prop-roots)
  - `bamboo_block`, `stripped_bamboo_block`, `bamboo` -> add BAMBOO (bamboo clusters)
- DROP ALWAYS (pale-garden markers): `creaking_heart`, `open/closed_eyeblossom`, `pale_moss*`,
  `pale_hanging_moss`.
- DROP for v1 (optional later): `vine` (306 files, directional state), `cocoa`, decorative
  fences/buttons/candles/wool/carpets.

## Curation (use all but pale garden)

Re-include the tall pines (user reversed the earlier exclusion). Drop ONLY:
- the `vanilla-plus` "Pale Oaks" community + `pale oak` folder files (the pale garden trees)
- giant mushrooms (`VN+ Giant Mushrooms`)
- pale-garden marker blocks (above)
Everything else loads, including tall old-growth pines and 21+ block emergents (scale-gated).

## Performance

Each Meld cell is a separate arnis process. Load ONLY the active realm pack + vanilla-plus
(~300 schems) per process, selected by `--tree-realm`. ~300 gzip+NBT parses ~= 0.1 s one-time,
negligible. Memory a few MB of voxel lists. No per-cell library cost beyond startup.

## Wiring

- `src/args.rs`: keep `--tree-pack <DIR>` (library root); add `--tree-realm <code>`.
- `src/region.rs` (new): lat/lon -> realm; community tagger (name keywords -> habitat axes);
  per-cell community + species + size selection.
- `src/tree_library.rs`: load per realm; index by community/species/size; weighted pick.
- `src/schematic.rs`: extend `map_block`; spacing-grid filter helper.
- `src/element_processing/tree.rs`: stamp via region pick (already stamps inside `create_of_type`).
- Meld `light-meld/src/arnis_cmd.py`: compute realm from selection center, pass `--tree-realm`.
- Meld `light-meld/web/index.html`: "Tree biome" dropdown - Auto (by location, default) / vn+ / 9 realms.
- Bundle: copy all 10 packs to `light-meld/tree-packs/` (1.6 MB), gitignored.

## Staging

- S1 palette + curation: expand `map_block`, add block defs, drop pale-garden, re-include pines. Low risk.
- S2 spacing grid filter. Self-contained, fixes the touching-trunk screenshots. Low risk.
- S3 realm + community engine (`region.rs`) + per-realm load + `--tree-realm`. Core. Medium risk.
- S4 Meld wiring + UI dropdown + bundling. Low risk.
- S5 tune weights/density per community from the data. Iterative.

## Confidence

- Inventory / community extraction: 99% (measured).
- Palette expansion + curation: 95% (all targets are real MC blocks).
- Spacing grid filter: 88% (deterministic, tile-invariant; needs in-world tuning of S per size).
- Realm-from-latlon: 90% (coarse boxes; vn+ fallback covers gaps).
- Community-from-signals via name-keyword tagging: 82% (auto-tag is good; ~10-15 communities need
  manual habitat overrides; edge blending needs a play-test pass).
- Overall system, first playable: ~85%. Iterative tuning (S5) expected.
