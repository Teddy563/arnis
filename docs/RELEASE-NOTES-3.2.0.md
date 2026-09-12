# arnis fork 3.2.0

*(pairs with Meld 1.9.9)*

This release brings the upstream 3.2.0 work into the fork: two new worlds that are not
Earth, Voxy LOD pregeneration, 18 blocks, a locale, and four correctness fixes. Default
settings render the same world 3.1.8 did — everything new is behind a flag that is off
unless you ask for it.

---

## Moon and Mars

`--body moon` and `--body mars` build the real surface from NASA's own elevation rasters —
LRO LOLA for the Moon, MGS MOLA MEGDR for Mars — each at that body's fixed scale.

They carry no map data, so a body forces terrain-only and switches off everything that
decorates an Earth surface. That includes this fork's own additions: caves, snow, scatter,
field textures, tree packs and props, none of which upstream has to think about. Scale
validation only applies to Earth, and the world clock starts at midnight so the sky reads
as space rather than as an overcast afternoon.

CLI only — the desktop GUI has no body picker. Meld 1.9.9 has one.

```
arnis --bbox=... --path=... --body moon
```

## Voxy LOD pregeneration

`--voxy-lod` builds the [Voxy](https://modrinth.com/mod/voxy) mod's LOD cache while the
world is being written, so the world renders to the horizon the first time you join
instead of needing `/voxy import current` and a long wait.

Java only, and it implies `--bake-lighting`: unlit LOD terrain renders black.

Two things about it are specific to this fork. The pyramid aggregates a 2×2 of chunks per
Morton column and flushes level *n* every 4ⁿ columns, so it is only correct if chunks
arrive in Morton order — with the flag on, the region writer walks the region that way,
and with it off the order is the one it has always had, so a default render stays
byte-comparable with the ones before it. And chunks the generator never filled are written
as a base plane by the second pass; those are fed to the LOD as well, or the pyramid has a
hole everywhere the world is bare, which in a rural region is most of it.

Costs extra time and disk per world. The cache lands in `<world>/voxy/` and is rebuilt from
scratch each run, so a regenerated world never inherits half of an older one.

```
arnis --bbox=... --path=... --voxy-lod
```

## 18 more blocks

End stone, purpur, crimson and cherry wood, dark prismarine, waxed cut copper slab, pale
oak trapdoor, coal block, blackstone slab and iron door, with their Luanti and Bedrock
mappings.

They are numbered from 450, above this fork's own ceiling of 449. This fork renumbered the
whole block table years ago — `LEVER` moved from 256 to 16, `MAGMA_BLOCK` from 18 to 256 —
so upstream's ids name *different blocks* here and can never be taken as-is. Seating the
new ones above the ceiling means no id already written into a world, or into Meld's region
cache, moves by one.

## Fixed

- **Magma no longer appears on lake and sea floors.**
- **Bedrock biome padding uses 0xFF, not 0.** Zero is a valid biome id, so the old padding
  read back as real biome data and the game kept it instead of regenerating the column.
- **The quarry ore roll no longer panics on deep terrain.** `0..100 + absolute_y` is an
  empty or inverted range once the floor drops below −100, which this fork reaches by
  design through `--min-y` and `--disable-height-limit`.
- **Two entities sharing a block cell no longer take each other's place** at the tile-halo
  merge — two signs on opposite faces of one post, say. The dedup key now carries the
  entity's UUID where it has one.
- **An iron door's upper half is written as an upper half on Bedrock.**
- **Trees, park vegetation and natural fill stay off sealed surfaces.** A block check cannot
  tell a paved area from natural ground - `surface=dirt` reads as dirt like any field, and a
  pitch drawn after the park around it has not been painted yet when the park scatters its
  vegetation. The columns owned by roads, pitches, courts, playgrounds and parking are
  resolved once from the element list, before anything is placed. A mapped `natural=tree`
  keeps its paving exception.
- **Roads are separable from stone in `--map-preview`.** Gray concrete powder is the primary
  road surface and had no palette entry, so it rendered as stone's grey.
- **Overture roof strings are interned**, taking three allocations per building off a fetch
  that runs to hundreds of thousands of rows on a large area.

## Also

Georgian localisation (ka-GE), and a refreshed baked Wikidata 3D model index.

---

## How much of upstream this is

85% of what a user would name, 39% by commit count, and the difference is one feature:
upstream's building facades are 28 commits and 51 668 lines - 72% of its whole delta.
The measurement, three ways, is in [UPSTREAM-3.2.0-COVERAGE.md](UPSTREAM-3.2.0-COVERAGE.md).

## Not taken from upstream, and why

This release was **ported, not merged**. The first attempt was `git merge upstream/main`:
76 conflicted files, and resolving them produced a tree that could not be made to build —
594 errors, 239 in `buildings.rs` alone. Git merges regions only one side changed silently,
so upstream's rewritten wall renderer landed in regions this fork had not touched while the
conflict markers were resolved fork-side, and the file ended up holding half of each.

So the fork is the base and each upstream change was applied deliberately, compiling after
every step. What did not fit is honest about not fitting:

| Upstream feature | Why not here |
|---|---|
| Mapillary and preset building facades | 45 634 lines, 63% of upstream's whole delta. Upstream rebuilt its wall renderer around a `FacadePlan`; this fork rebuilt the same renderer around its architectural-era and window-frame grammar, with interiors, loot and signage anchors written against it. Alternatives, not layers. |
| Overture vector-tile transport | Upstream split `overture.rs` into a module directory; this fork has 1 028 lines of its own in that file, including the prewarm path Meld drives. A hand-merge, not a port. |
| Jet bridge prop | Needs a free-yaw schematic routine this fork does not have. |
| `gui/js/logging.js` | Routes through a `gui_log` command this fork's GUI does not define. |
| Upstream's custom world name | Duplicate: this fork already has `--level-name`. |

Deferred to 3.3.0, not abandoned. The measured cost of each is in
[UPSTREAM-3.2.0-PORT-AUDIT.md](UPSTREAM-3.2.0-PORT-AUDIT.md); what landed and
where is in [RELEASE-3.2.0-INVENTORY.md](RELEASE-3.2.0-INVENTORY.md).

## Upgrading

Nothing to do. No block id moved, no existing flag changed meaning, and no default changed.
A world generated with 3.1.8 opens and extends exactly as before.

Meld 1.9.9 drives the new options and hides the ones a given binary does not advertise, so
an older generator keeps working with it unchanged.

## Verification

- `cargo test`: 587 passed, 0 failed, 8 ignored
- `cargo check --all-targets`: clean
- Moon world confirmed using NASA PDS LOLA data end to end
- `--voxy-lod` confirmed writing a complete cache (`CURRENT`, `IDENTITY`, `MANIFEST`, WAL)
  from a real two-region render
