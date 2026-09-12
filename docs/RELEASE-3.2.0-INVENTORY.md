# Fork 3.2.0 / Meld 1.9.9 — what landed, and where

Written for: whoever prepares the release notes and the build, and for the next person who
looks at upstream and wonders why some of it is here and some is not.

Upstream reference: `louis-e/arnis@338a1d28` (tag `v3.2.0`), 133 commits ahead of the merge
base `c7b5f19d`. The full cost analysis is in [UPSTREAM-3.2.0-PORT-AUDIT.md](UPSTREAM-3.2.0-PORT-AUDIT.md).

## How this was done, and why it matters

The first attempt was `git merge upstream/main`. It produced 76 conflicted files, and
resolving them produced a tree that could not be made to build: 594 errors, 239 in
`buildings.rs` alone.

The reason is worth writing down. Git splits a file into regions only one side changed
(merged silently) and regions both changed (marked as conflicts). Reviewing only the
conflicts is not enough. Upstream rebuilt its building wall renderer, and much of that
rewrite landed in regions this fork had not touched — so git merged upstream's new
architecture in without asking, while the conflict markers were resolved in favour of the
fork's. The file ended up holding half of each.

This release was therefore **ported, not merged**: the fork is the base, and each upstream
change was applied deliberately, compiling after every step. Every commit below is
independently green.

## arnis fork — `release/3.2.0-upstream-merge`

| Commit | What |
|---|---|
| `79782588` | The audit: measured scope of all 133 upstream commits |
| `c2cab150` | 18 block additions, Georgian locale, refreshed Wikidata index |
| `52df05ce` | Four correctness fixes |
| `9469891e` | Moon and Mars (`--body moon\|mars`) |
| `0ad7495b` | Version bump to 3.2.0 + changelog |
| (this commit) | Voxy LOD pregeneration (`--voxy-lod`) |

### Files touched

**New:**
- `src/voxy/` — the LOD pyramid, its RocksDB-shaped writer and block mapper
- `assets/voxy/MANIFEST.golden` — the manifest prefix the database is sealed with
- `src/celestial.rs` — the bodies, their fixed scales, their surface palettes
- `src/elevation/providers/planetary.rs` — NASA PDS rasters (LRO LOLA, MGS MOLA MEGDR)
- `src/gui/locales/ka-GE.json` — Georgian

**Changed:**
- `src/args.rs` — `--body`, `apply_body_defaults`, `DEFAULT_WORLD_TIME`/`MIDNIGHT_TICKS`,
  scale validation gated on Earth
- `src/block_definitions.rs` — 18 constants at ids 450–467 plus their Java name arms
- `src/luanti_block_map.rs` — the matching Luanti arms
- `src/bedrock_block_map.rs` — purpur/dark-prismarine states, iron-door half fix
- `src/elevation/selector.rs` — `SourceMode` replaces the `force_aws` bool
- `src/elevation/mod.rs`, `src/elevation_map.rs`, `src/ground.rs` — `SourceMode` threaded
- `src/element_processing/landuse.rs` — ore-roll overflow clamp
- `src/water_depth.rs` — magma off water floors
- `src/world_editor/bedrock.rs` — biome padding 0 → 0xFF
- `src/world_editor/java.rs` — entity dedup keyed on UUID; the Voxy feed (Morton chunk
  order, shared span/lighting, filler chunks ingested too)
- `src/biome.rs` — `chunk_biome_names` split out of `build_chunk_biome_nbt`, so the chunk
  file and the LOD are written from one set of names
- `src/world_editor/mod.rs`, `src/data_processing.rs`, `src/gui.rs` — the writer threaded
  through and sealed after the Java save
- `src/main.rs` — `mod celestial`, `apply_body_defaults` before the derived flags
- `src/gui.rs` — `body: Earth` in the GUI's Args (no body picker in this fork's GUI)
- `assets/wikidata_3d_models.json`, `Cargo.toml`, `tauri.conf.json`, `CHANGELOG.md`

### The one decision that constrains everything downstream

This fork renumbered the whole block table years ago — `LEVER` moved from 256 to 16,
`MAGMA_BLOCK` from 18 to 256. Upstream's ids therefore name *different blocks* here and can
never be taken as-is. Upstream's 18 new constants were re-seated at **450–467**, above this
fork's own ceiling of 449, so no id already written into a world or into Meld's region cache
moves by one. Any future port from upstream has to do the same.

## Meld — `release/1.9.9-arnis-3.2.0`

| Commit | What |
|---|---|
| `1d60b38` | 1.9.9: drive the 3.2.0 options, gated on what the binary advertises |
| `59bd022` | Classic/Enhanced switch, facade-source dropdown, Show cells moved |

### Files touched
- `src/arnis_cmd.py` — `upstream_3_2_flags()`, the `_UPSTREAM_3_2_OPTIONS` table, the
  Classic gate
- `src/project.py` — defaults for every new key, including `gen_mode_32: "classic"`
- `src/presets.py` — `mapillary_token` excluded from shared presets
- `server.py` — `/api/arnis-caps`, `MAPILLARY_TOKEN` into the child environment,
  `mapillary_token` excluded from the world metadata sidecar; the dead duplicate capability
  probe removed
- `meld_app.py` — `--arnis-caps` and `--print-arnis-cmd`
- `web/index.html` — the 3.2.0 settings drawer, Classic/Enhanced, facade source, Show cells
- `tests/test_upstream_3_2_flags.py` — 46 tests

### The compatibility mechanism

Meld emits a 3.2.0 flag only when the binary advertises it in its own `--help`
(`arnis_supports()`), and only in Enhanced mode. Against a 3.1.8 binary every probe answers
False and the command line is byte-identical to 1.9.8's. There is no version sniffing: a
locally built or side-loaded binary does not report a version honestly enough to branch on.

`meld --arnis-caps` prints which options the deployed binary accepts — the direct answer to
"why does this toggle do nothing".

## Security note for the release

The Mapillary token is a credential and is handled as one: passed to the generator through
the child's environment (`MAPILLARY_TOKEN`), never on the command line, because argv is
readable by other processes. It is stripped from shared presets and from the
`meld-world.json` sidecar written into world folders — both things people hand to others.

## Deferred to 3.3.0, with reasons

| Upstream feature | Why not now |
|---|---|
| Mapillary + preset building facades | 45 634 lines, 63% of upstream's delta. Upstream rebuilt the wall renderer around a `FacadePlan`; this fork rebuilt the same renderer around its era/window-frame grammar. Alternatives, not layers. |
| Overture vector-tile transport | Upstream split `overture.rs` into a module directory; this fork has 1 028 lines of its own in that file, including the prewarm path Meld drives. Hand-merge, not a port. |
| Jet bridge prop | Needs a free-yaw schematic routine this fork does not have. |
| `gui/js/logging.js` | Routes through a `gui_log` command this fork's GUI does not define. |
| Upstream's custom world name | Duplicate: this fork already has `--level-name`. |

## Verification at the time of writing

- `cargo check --all-targets`: clean
- `cargo test --no-default-features`: 558 passed, 0 failed, 8 ignored
- Meld: 588 passed
- `--body` confirmed present and documented in the built binary's `--help`
