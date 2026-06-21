# Plan: geography-driven trees + schematic tree library (Arnis fork)

Local design doc. No code yet. No commits to main, no co-authoring. Captures the
full design from the audit + discussion so we can resume tomorrow.

## 0. The one big idea
Two SEPARATE axes. Do not conflate them:
- **Species CHOICE** = which tree goes where (oak vs spruce vs willow). Driven by geography.
- **Tree SHAPE** = how the tree looks (procedural rings vs a stamped schematic).

They are orthogonal. Geography choice + size tiers + rotation apply to ALL shape modes.

---

## 1. How trees/forest/flora work today (audit summary)
- `tree.rs`: 14 species (oak, spruce, birch, dark-oak, jungle, acacia, cherry, tall-oak,
  pine, bush, azalea, willow, flowering-oak, mangrove), 1-5 variants each. Each tree is a
  trunk column + concentric-ring canopy, ~150-800 blocks (mean ~350).
- Species pick today = a FIXED global percent table (oak 20%, spruce 12%, birch 12% ...)
  seeded by `coord_rng(x,z,0)`. Variant + leaf detail from `coord_rng(x,z,1)`.
  OSM-tagged trees override: species > genus:wikidata > genus > leaf_type.
- Forests (`natural=wood`, `landuse=forest`, ESA tree cover): noise density,
  `value_noise(x,z,32)` -> `threshold = max(5, 60 - density*45)`, spawn when `rng%threshold==0`.
  No thinning pass, no per-species spacing.
- Deciduous vs coniferous: ONLY from OSM leaf_type/species tags. NO climate/latitude logic.
- Flora: per-cell single-block scatter by land class + ESA fallback.
- Fauna: NONE. No mobs/entities. All static blocks.
- Everything deterministic + tile-invariant (position-seeded `coord_rng`).

### Biggest gaps
1. Repetition: canopies are math rings, big forests read as a few blob shapes tiled out.
2. No climate awareness: tropical and boreal maps get the same oak/spruce/birch mix.
3. Flat single-block flora, no undergrowth layering, no fauna.

---

## 2. Geography-driven species choice (Phase 1, the cheap win)
Reachable signals (all per-coordinate, shared across tiles, so tile-invariant):

| Signal | Source in code | Now? | Rule | Species bias |
|---|---|---|---|---|
| Latitude boreal | `args.master_origin_lat` (+ snow-line fn uses it) | Yes | `|lat|>55` | Spruce/Pine heavy |
| Latitude temperate | same | Yes | `35-55` | Oak/Birch/Dark-oak |
| Latitude tropical | same | Yes | `<23` | Jungle/Acacia/Mangrove |
| High elevation (montane) | `Ground.level()` vs latitude-adjusted snow-line Y | Yes | near snow line | Spruce/Pine |
| Above tree line | `Ground.level()` vs snow line | Yes | at/above | Bush / none |
| Steep slope | `Ground.slope(coord)` | Yes | `slope > N` | Pine/Spruce, fewer broadleaf |
| Near water | `big_water_field` DT (our water work) or LC_WATER neighbour | Needs small plumb | DT within k blocks | Willow (temp) / Mangrove (trop) |
| Wetland/mangrove class | `Ground.cover_class()` LC_WETLAND/LC_MANGROVES | Yes | class match | Willow/Mangrove |
| Shrubland/heath | `cover_class` LC_SHRUBLAND | Yes | class match | Bush/Azalea |
| Dry grassland + low lat (savanna) | `cover_class` LC_GRASSLAND + lat | Yes (approx) | grassland and `lat<30` | Acacia |
| OSM tag explicit | `natural.rs` species/genus/leaf_type | Yes (already) | tag present | Exact species (wins over all) |

How it plugs in: replace the fixed percent table with a geography-WEIGHTED table built from
the rows above, still sampled by `coord_rng(x,z,0)`. Same determinism + tile-invariance, the
weights just shift by place. Mountain forest -> spruce-heavy; shoreline -> willow/mangrove;
tropical map stops looking Central-European. Small code change, no new assets.

---

## 3. Schematic tree stamp (Phase 2, the shape upgrade)
Reuse the BACK HALF of the existing `models_3d/` pipeline (placement, ground-snap,
region deferral, stream-to-disk guards). Do NOT reuse the mesh fetch / triangle /
color-tone-mapping front end. Schematics are already voxel grids of block IDs.

### Per-tree selection pipeline (all off one position seed, different salts)
| Pick | Source | Salt | Notes |
|---|---|---|---|
| Species | geography weights (section 2) | 0 | which tree |
| Size tier | scale-aware weights (section 4) | 1 | small/medium/big |
| Variant | `% variants_in(species,size)` | 2 | which schematic |
| Rotation | `% 4` | 3 | 0/90/180/270, coordinate swap around trunk base |

Rotation is free 4x variety: 5 schematics x 4 rotations = 20 looks. The 3D pipeline already
does arbitrary yaw; 4 cardinals is the easy subset (swap/negate dx,dz around the anchor).

### Library layout
`assets/trees/<species>/<size>/<n>.schem` (gzipped) + a manifest (species, size, variant,
voxel bounds, y_offset for ground snap). Parse once at startup into
`HashMap<variant_id, Vec<(dx,dy,dz,Block)>>`.

---

## 4. Size tiers + scale-aware weights
| Tier | Height @ 1:1 | Use |
|---|---|---|
| small | 4-6 blocks | undergrowth, young trees, small-scale maps |
| medium | 8-12 blocks | workhorse, most common |
| big | 15-25 blocks | hero canopy, rare |

Size weights by scale band (same gate idea as the small-scale water work):

| Scale | tree height (blocks) | sizes | small / med / big |
|---|---|---|---|
| `>= 0.5` (1:1, 1:2) | 4-25 | small+med+big | 20 / 60 / 20 |
| `0.2 - 0.49` (1:5) | 4-8 | small+med | 60 / 40 / 0 (no big, would dwarf shrunk terrain) |
| `0.1 - 0.2` (1:10) | 2-4 | small only | 100 / 0 / 0 |
| `< 0.1` | 1-2 | procedural bush (no schematic) | n/a |

Medium common at full scale; small/big are the rare tails; small maps drop big; smallest maps
use only small.

### Smallest tree that still looks good
- Scale `>= ~0.3`: schematics shine (trees >= 6 blocks, detail visible).
- Scale `0.1-0.3`: one "micro" schematic per species, roughly 3x4x3 (1 trunk + 3-wide 2-tall cap). That is the floor that still reads as a tree.
- Scale `< ~0.1`: skip schematics, use the existing procedural Bush (log_height 0 + small leaf cluster). Below ~2 blocks a schematic adds nothing over a blob.

---

## 5. Three styles, user-selectable (one engine, swappable library)
The stamp engine is one thing; the library it loads changes per mode.

| Mode | What | Source | Cost |
|---|---|---|---|
| `default` (current) | Arnis procedural concentric-ring trees | code formula | none, ships today |
| `vanilla` | Minecraft-native tree shapes | schematic pack captured from real MC trees (grow + WorldEdit-export .schem) | author once per species |
| `custom` | fancier hand-authored / stylized | custom schematic pack | author, the expensive one |

- "Vanilla-like" = a schematic pack whose CONTENTS are actual vanilla MC trees. Same engine as custom; only the files differ.
- Expose as `--tree-style default|vanilla|custom` (CLI) + a Meld UI dropdown (like the buildings toggle).
- Geography choice, size tiers, rotation apply to ALL three modes (procedural variants are size-tiered too).
- Can we run custom AND vanilla together? Technically yes, but mixing two packs in one
  generation adds seam-config risk for little gain. Cleaner: ONE mode per generation, user-chosen.

---

## 6. Tile-invariance contract (the one gotcha)
Every pick (species, size, variant, rotation) MUST be seeded from the tree's UNCLIPPED world
position, not the tile-clipped position. If a straddling tree seeds from clipped coords on one
tile and unclipped on the other, they pick different trees -> visible seam. Procedural trees
already solve this (`coord_rng` + unclipped bounds); schematic picks must use the identical
seed. Ground snap is already deterministic (shared Ground, bilinear DEM). Test: generate two
adjacent tiles, binary-diff a straddling tree, must be byte-identical.

---

## 7. Performance (non-issue)
- ~200 trees/km^2 x ~350 blocks x ~0.5 us = ~35 ms/km^2. Negligible vs 2-5 s for DEM + OSM.
- Schematics are LOCAL files, zero fetch. ~2-4 KB compressed each; 20-variant library ~80 KB
  resident, ~1 MB even at 300. Ground snap adds ~10 ms/km^2.
- Schematic trees place no slower than procedural (both tight `set_block` loops). CPU/RAM is
  not the cost. Authoring time is.

---

## 8. Approaches compared
| Approach | Accuracy to a designed tree | Speed/tree | Memory | Authoring | Tile-inv | New code |
|---|---|---|---|---|---|---|
| A. Procedural (today) | Low (synthetic rings) | ~350 set_block | tiny | params | Yes | none |
| B. Direct schematic stamp | 100% exact | SAME as A | voxel array (KB) | high | Yes if seed is position-based | loader + place fn + prescan |
| C. Translate schematic -> procedural | Low (rings cannot express branching, lossy) | fast | tiny | worst of both | Yes | converter |

Verdict: B is accurate at parity speed, only costs a little RAM. C is fast but throws away the
shape you authored. Skip C.

---

## 9. Recommended phased plan
- **Phase 1 (cheap, do first): geography-driven species choice.** Replace the fixed percent
  table with the weighted table in section 2. Biggest realism jump, no new assets, tile-invariant.
  Plumb `big_water_field` (or an LC_WATER neighbour scan) to the tree site for water proximity.
- **Phase 2: schematic stamp engine + the 3 styles.** New `.schem` loader -> `Vec<(dx,dy,dz,Block)>`,
  `place_schematic_tree(editor, voxels, anchor_x, anchor_z, ground_y, rotation)` reusing the
  models_3d ground-snap. Wire species/size/variant/rotation picks (section 3) into the existing
  position-seeded path. `--tree-style` flag, `default` as default. Seam test + visual QA.
- **Phase 3: grow the library** only if it clearly improves the look. Start with 2-3 hero species
  (oak/spruce/birch), small/medium/big + a micro, ~12-15 schematics.

### Files this touches
- `tree.rs` (species/size/variant selection, size-tiered procedural variants)
- `element_processing/natural.rs`, `landuse.rs` (tree placement sites, geography signals)
- `ground.rs` (already exposes cover_class/slope/level; latitude via args)
- `data_processing.rs` (prescan: load schematic cache, plumb big_water_field to tree site)
- `args.rs` (`--tree-style`), Meld UI (dropdown), `arnis_cmd.py` (pass the flag)
- new `models_3d/schematics/` (loader) + `assets/trees/` (library + manifest)

### Flags / UI
- `--tree-style default|vanilla|custom` (default = procedural).
- Meld UI dropdown next to the buildings toggle.

---

## 10. Pros / cons
PROS: reuses proven infra; real variety at no perf cost; tile-invariant by inheritance;
mixable per species; bundled = no network; size+rotation give big variety from a small library.
CONS: authoring burden is the real cost (full coverage = 200-300 schematics); seam fragility if
variant pick is not position-seeded; a prescan refactor (variant known before placement);
config complexity (new flag + manifest + .schem parser); does not fix climate/density/fauna gaps
on its own (Phase 1 does the climate part).

## 11. Confidence
- Phase 1 (geography species): high, ~85%. Small change, all signals in hand.
- Phase 2 (schematic engine + styles): ~72%. Engine + perf solid; risk is authoring + the
  unclipped-seed seam contract.
- Skip C (translate-to-procedural): certain.

---

## 12. Vanilla source: almic/VanillaTrees + .nbt vs placer-port
almic/VanillaTrees (2023): a WorldPainter brush collection PLUS ~700-2500 raw `.nbt`
STRUCTURE files of Minecraft 1.15 vanilla trees/plants/structures. Mojang IP (no attribution
required, "adhere to MC terms").

Loader cost is near zero: Arnis already depends on `fastnbt` + `flate2` (+ `zip`, `nbtx`). A MC
`.nbt` structure = gzipped NBT with a `palette` (block states) + `blocks` (pos + state index).
A ~50-100 LOC reader gives `Vec<(dx,dy,dz,Block)>`, easier than `.schem`. So the loader should
target the `.nbt` STRUCTURE format as primary (MC-native, what these use, reuses our NBT stack);
`.schem` optional.

Catches with bundling VanillaTrees:
- LICENSE: these are Mojang's own tree designs. Bundling Mojang-derived NBT into an open-source
  repo is a redistribution gray area. Do NOT bundle. Support a user-supplied `--tree-pack <dir>`
  so users drop VanillaTrees or their own captures in locally.
- Version-locked to 1.15 (no cherry, mangrove, pale oak); needs block-state mapping to our defs.
- Finite set (mitigated by 4x rotation + size tiers).

Better "vanilla, any version" routes:
- (a) BEST clean-room: PORT Minecraft's tree feature placers. Vanilla trees are NOT stored as
  NBT in the game; they are GENERATED by TrunkPlacer + FoliagePlacer from JSON in
  `data/minecraft/worldgen/configured_feature/`. Those params are public data. Reimplementing the
  placer math gives TRUE vanilla shapes, ANY version, infinite variety, deterministic, and bundles
  ZERO Mojang assets (we ship math, not their files). This is the proper `vanilla` mode and dodges
  the license issue. Arnis's current procedural is a crude version of this; make it faithful.
- (b) Self-capture from latest MC via the structure block -> `.nbt` (gets newest species). You
  generate it, used locally via `--tree-pack`.
- (c) Other community packs: same license caveat, check each.

Refined modes:
- `default`: current simplified procedural.
- `vanilla`: faithful MC placer port (procedural, clean-room, any version). RECOMMENDED. Optionally
  also accept a user `--tree-pack` of `.nbt`.
- `custom`: OUR own bundled `.nbt`/`.schem` designs (no license issue, we authored them).

---

## 13. Concrete pack found: "vanilla-plus" (VN+) in Downloads
`C:\Users\LEGION\Downloads\vanilla-plus`: 99 `.schem` (Sponge) + 17 `.layer` (WorldPainter, ignore).
- Version: DataVersion 3953 = **MC 1.21.4** (modern, not version-locked like almic 1.15).
- Species folders (schems): acacia 8, azalea 7, birch 8, dark oak 12, jungle 17, oak 8,
  pale oak 15, swamp 9, taiga 15 (taiga = spruce/pine, swamp = swamp oak).
- Size spread by Height: min 3, median 11, max 16. Buckets small(<=6)=15, medium(7-12)=49,
  big(13+)=35. This already matches the plan's "medium common, small+big rare" shape, so size
  tiers come FREE by bucketing on Height.
- Blocks used: standard logs/leaves/wood + azalea/flowering_azalea, vine, cocoa, short/tall grass,
  plus 1.21.4 exotics (pale_oak_*, creaking_heart, open/closed_eyeblossom, pale_moss_block/carpet,
  pale_hanging_moss).
- License: NONE found in the folder. Source unknown.

Block mapping needed (~10 entries):
- ADD real defs (Arnis is 1.21.x, these are real blocks): PALE_OAK_LOG, PALE_OAK_LEAVES, OAK_WOOD,
  FLOWERING_AZALEA_LEAVES, VINE, COCOA.
- FALLBACK the un-renderable: CREAKING_HEART -> PALE_OAK_LOG, OPEN/CLOSED_EYEBLOSSOM -> a flower,
  PALE_MOSS_BLOCK/CARPET/PALE_HANGING_MOSS -> MOSS_BLOCK/MOSS_CARPET/vine. The 3D pipeline already
  has an unknown-block fallback pattern (magenta sentinel -> GLASS) to copy.
- HAVE already: all core logs (oak/birch/dark_oak/jungle/acacia/spruce/cherry/mangrove),
  azalea_leaves, short/tall grass.

Use it via `--tree-pack <dir>` (user-supplied, local), do NOT bundle (license unknown). For a
shippable bundled `vanilla` mode, prefer the clean-room placer-port (section 12a). This pack is the
fastest path to great-looking trees in the user's own Meld right now.

Sponge `.schem` loader sketch: gunzip (flate2) -> NBT (fastnbt) -> read Width/Height/Length (shorts),
Palette (block-state string -> int), BlockData (varint indices, YZX order); map each block string
(strip `[state]`) to our Block via the mapping table; emit Vec<(dx,dy,dz,Block)>. ~100 LOC, reuses
the 3DMR voxel placement + ground-snap.

---

## 14. Region-aware realistic tree system (full design)

Goal: most accurate trees per place, region packs added over time, no tile seams, no buildings
cut by branches. Default = the curated vanilla-plus pack.

### 14.1 Curated rules for the default (vanilla-plus) pack
Discard always: pale oak / pale garden (pale_oak, creaking_heart, eyeblossom, pale_moss*), giant
mushrooms, and the tall pines `Pinus_piceoides*` (too high). Keep spruce `Picea_*`.

| Context (signal) | Species folder(s) | Pack files |
|---|---|---|
| Mountain: high elevation OR steep slope | spruce (taiga) | `taiga/Picea_*` only (NOT Pinus) |
| Upper-mid slope (transition down) | spruce + oak patches (blend) | `taiga/Picea_*` + `oak/Quercus_generica*` |
| Lowland temperate forest | oak + birch + dark oak | `oak/`, `birch/`, `dark oak/` |
| Savanna: low latitude + dry grassland | acacia | `acacia/Vachellia_*` |
| Wetland / near water | swamp oak | `swamp/Quercus_vinifera*` |
| Shrubland / heath | azalea / bush | `azalea/` + procedural bush |
| Tropical wet | jungle | `jungle/` |

Mountains read as taiga (spruce) up high, blending to oak patches as elevation drops. Exactly the
"taiga with patches of oak going lower" look. No pale garden, no mushrooms, no tall pines.

### 14.2 Region resolver (future packs)
- `resolve_region(lat, lon) -> RegionKey` from a coarse continent/ecozone table; `--tree-region <key>`
  override + a Meld UI dropdown.
- Each region = a manifest (TOML/JSON): pack dir + context->species rules (like 14.1) + size buckets +
  discard list. Default region = vanilla-plus + the 14.1 table.
- Planned regions: e_north_america, w_north_america, south_america, australia, europe, africa,
  mountain. Adding a region = drop a pack dir + a manifest; resolver routes by lat/lon. No code change.
- So the code is written generic now (rules are DATA in a manifest, not hardcoded), the default manifest
  encodes 14.1, future packs are pure data drops.

### 14.3 Mixed-forest blend (not uniform stands)
- Keep existing forest-fill noise (`value_noise scale 32`) for WHERE a tree goes.
- Add a low-freq blend noise (scale ~96) so a stand mixes species organically. Species weight shifts by
  elevation: high -> spruce-dominant, low -> oak-dominant, with the blend noise giving patchiness. So a
  mountain forest is mostly spruce with oak pockets, fading to oak/birch lowland. All position-seeded.

### 14.4 Tile-invariance + cross-tile stamping (THE seam fix)
- Every pick (species, size, variant, rotation) seeded from the UNCLIPPED node position; region is a
  global constant. Ground-snap is shared. So picks agree across tiles.
- CRITICAL new work: a tree whose bbox crosses a tile boundary must be stamped by BOTH tiles (each
  clipped to its own region), exactly like the 3D-model region deferral the fork already does. If only
  the anchor tile stamps it, the canopy is cut at the seam. place-if-absent is order-independent, so both
  tiles writing the shared tree produce identical blocks. This is the main risk item.

### 14.5 Building collision (no cut branches)
- (a) Footprint gate: never anchor a tree whose trunk cell is in a building footprint
  (`BuildingFootprintBitmap.contains`, already used by procedural trees). Also skip road_mask + water.
- (b) place-if-absent for EVERY schematic voxel: write only into world-AIR via
  `set_block_if_absent_absolute` (exists, used in ground_gen). A building/road/existing block is non-AIR,
  so an overhanging branch is simply skipped there. Never overwrites above OR below an existing block.
- (c) Loader DROPS AIR voxels from the schematic (air = empty space, never written).
- Net: branches stop at walls/roofs instead of cutting through them, with zero extra masks.

### 14.6 Performance (easy)
- Load: parse ~99 schems once at startup (gunzip + fastnbt), ~100 ms total, cached. Region packs add
  ~100 ms each, one-time.
- Place: ~350 blocks/tree x ~200 trees/km2; place-if-absent adds a read-before-write, so ~35-70 ms/km2.
  Negligible vs the 2-5 s DEM + OSM already spent.
- Memory: ~few hundred KB per pack resident (under ~1-2 MB even with several region packs).
- Cross-tile: only boundary trees stamped twice (the tile perimeter), negligible.
- Bottom line: low single-digit % generation overhead. The real cost is curating which schems per region.

### 14.7 Confidence
- Curated 14.1 rules + default pack: ~85%.
- Region resolver as DATA manifests: ~80% (architecture solid; the ecozone lat/lon table is coarse, refine later).
- Building collision via place-if-absent + footprint gate: ~90% (primitives exist).
- Tile-invariance + cross-tile canopy stamping: ~70% (the seam-crossing canopy is the real risk; the 3D
  pipeline proves it is solvable but needs care).
- Overall: ~78%.

---

## 15. Rarity map + patch/sprinkle + region gating

### 15.1 Two-layer placement (patches AND random sprinkle)
1. PATCH layer: a low-frequency `value_noise_01(x,z, scale~96)` per species marks "stand zones"
   (this area is a birch grove / a dark-oak thicket / oak matrix). Gives clustered groves.
2. SPRINKLE layer: within a zone, the per-tree `coord_rng` pick uses the zone's weights, so even an
   oak matrix has the odd birch and a birch grove has the odd oak. Gives the random sprinkle.
Both are pure functions of (x,z) -> tile-invariant. Patches come from the noise, sprinkle from the
per-tree RNG; together they read like a real forest, not a uniform fill.

### 15.2 Rarity map (overall target share + how it is distributed)
LOWLAND TEMPERATE FOREST (oak-dominant, birch in patches, dark oak rare):
| Species | Overall share | Distribution |
|---|---|---|
| Oak | ~60% | the matrix, everywhere |
| Birch | ~25% | birch-grove patches (high in zones) + ~8% sprinkle in the matrix |
| Dark oak | ~6% | rare dense thicket pockets, near-zero sprinkle |
| Azalea / bush | ~9% | undergrowth random sprinkle |

MOUNTAIN (elevation gradient, spruce high -> oak low, blended by noise so the line is patchy):
| Band | Spruce (Picea) | Oak | Birch |
|---|---|---|---|
| High (near snow line) | 90% | 5% | 5% |
| Mid slope | 65% | 25% | 10% |
| Lower slope (transition) | 35% | 50% | 15% |

OTHER CONTEXTS:
| Context | Mix |
|---|---|
| Savanna (low lat, dry grassland) | Acacia 85% / bush 15% |
| Wetland / near water | Swamp oak 80% / willow (procedural) 10% / azalea 10% |
| Tropical wet | Jungle 80% / oak 10% / bush 10% |
| Shrubland / heath | Azalea 50% / bush 50% |

Tune the percentages later against a render; the structure (patch noise + sprinkle RNG + elevation
blend) is the load-bearing part.

### 15.3 Region gating (region trees only in their region)
Each region manifest is an ALLOW-LIST: it lists which species exist there and their context weights.
A species not in the active region's manifest NEVER spawns there. So acacia fires in Africa/Australia
savanna manifests, never in Europe. `resolve_region(lat,lon)` picks the manifest; `--tree-region`
overrides; UI dropdown. Future region packs (e/w North America, South America, Australia, Europe,
Africa, mountain) each ship a folder + manifest with their own allow-list, weights, and the 15.2-style
rarity map for that region. Adding a region = drop a folder + a manifest, no code change. The default
(vanilla-plus) manifest encodes 14.1 + 15.2.

### 15.4 Plumbing delta found in re-audit
`generate_natural` / `generate_natural_from_relation` need `ground: &Ground` (slope, cover_class,
level) and optionally `big_water_field` (water distance) added; the 3 call sites in `data_processing.rs`
already hold both. Latitude is `args.master_origin_lat`, elevation proxy is `editor.get_ground_level`.
Small, clean, same pattern as the water-depth scale plumb.

### 15.5 Updated confidence
| Piece | Confidence |
|---|---|
| Curated default rules (14.1) | 85% |
| Rarity map + patch/sprinkle (15.1-15.2) | 85% |
| Region gating via manifest allow-list (15.3) | 82% |
| Plumb ground+water into natural.rs (15.4) | 88% |
| Building collision (place-if-absent + footprint) | 90% |
| Cross-tile canopy seam | 70% (the cap; prove with a 2-tile diff) |
| OVERALL | ~80% |

---

## 16. Better tree placement: constant min-spacing + avoidance

GOAL: a better placement algorithm than Arnis has now. Spacing is CONSTANT (same in the actual
generation at every scale, NOT grown for small maps). Trunks are never touching; canopies may touch
lightly but never crammed. Trees never land on roads, water, or buildings, and branches never clip
into buildings.

Problem today: Arnis spawns a tree per qualifying cell / OSM node with no neighbour check, so trunks
end up adjacent (0-2 block spacing), oak-on-oak, and forest fill does not cleanly exclude roads/water.

Solution (three parts):
1. CONSTANT min-spacing blue-noise (jittered grid). One global grid of cell size = MIN_TRUNK_SPACING
   (a constant ~3-4 blocks, NOT scale-dependent). Each cell holds at most one tree, jittered within
   the cell by a position hash, gated by the section-15 forest density/patch noise. This guarantees
   trunks are always `>= ~MIN_TRUNK_SPACING` apart at every scale. With ~4-7 block-wide trees at 3-4
   spacing, canopies overlap only 1-2 blocks (light touch, like a real forest) while trunks stay clear.
   MIN_TRUNK_SPACING is one tunable constant in blocks, identical at 1:1 and 1:10.
2. AVOIDANCE masks, checked at the trunk cell before placing (skip the tree if any hit):
   - building footprint: `BuildingFootprintBitmap.contains` (already used by procedural trees).
   - road: `road_mask.contains` (plumb road_mask in; data_processing.rs already builds it).
   - water: `cover_class == LC_WATER` or `big_water_field.depth_at > 0` (plumb in).
3. CANOPY place-if-absent: every schematic voxel written with `set_block_if_absent_absolute` (writes
   only into AIR), air voxels dropped. So a branch overhanging a wall/roof/road is skipped at the
   occupied block and never overwrites it. Belt-and-suspenders with (2): (2) stops the trunk landing
   on a road/building, (3) stops the canopy clipping one.

Net: trunks evenly spaced (no oak-on-oak), nothing placed on roads/water/buildings, branches never
clip. Spacing is the SAME in the generation regardless of scale.

Tile-invariance: MIN_TRUNK_SPACING and the masks are global/deterministic, so the grid cells and the
mask lookups are identical from any tile; each cell's tree (spawn, jitter, species, size, rotation) is
a pure function of the cell index + unclipped position -> seam-safe.

Plumbing: add `road_mask`, `ground`/`cover_class`, and `big_water_field` to the tree site (same plumb
as 15.4; the callers in data_processing.rs already hold all of them).

Confidence: ~85% (jittered min-distance grid + mask checks are standard, use existing noise/hash/mask
primitives, tile-invariant by construction; MIN_TRUNK_SPACING wants one render to tune).
