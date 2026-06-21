# Contributing Meld fork fixes back to upstream Arnis

Status: fork `Teddy563/arnis` is **67 commits ahead, 0 behind** `louis-e/arnis` **v2.9.1**
(we branched at their 2.9.0). Not all 67 are ours: several are upstream changes we pulled
*in* (snow-line, parking visuals, byte-per-cell section storage, marina, and the
tree-apex-leaf cap, which is **already upstream**). The real job is slicing the genuinely-ours
fixes into **small, focused PRs**, easiest first.

---

## How a Pull Request actually works (quick primer)

A Pull Request (PR) is "please pull my change into your project." You never edit louis-e's repo
directly. You:

1. Already have a **fork** (`Teddy563/arnis`) and a clone with two remotes:
   `origin` = your fork, `upstream` = `louis-e/arnis`. (We have this.)
2. Make a **branch off `upstream/main`** (a *clean* base, NOT our fork's `main`, which carries
   all 67 commits), put **one** fix on it, push that branch to `origin`.
3. On GitHub, open a PR from `Teddy563/arnis:your-branch` -> `louis-e/arnis:main`.
4. The maintainer reviews, asks for changes, then merges.

What makes a PR get accepted (this is the whole game):

- **One focused change per PR.** A 20-line bug fix merges; a 4,000-line "here's my fork" does not.
- **Title + description say what AND why.** Lead with the user-visible symptom.
- **Reference the issue it fixes** ("Fixes #824") so it auto-links and closes on merge.
- **Keep our Meld-specific wording out.** Re-frame comments for a general Arnis user.
- **Pass their CI**: `cargo fmt --all -- --check`, `cargo clippy -- -D warnings`, `cargo test`.

The base-branch rule, concretely (do this for every PR):

```bash
git fetch upstream
git checkout -b fix/<short-name> upstream/main   # clean base = upstream, + nothing else
# ...apply just the one fix (cherry-pick or hand-patch)...
cargo fmt --all && cargo clippy -- -D warnings && cargo build --release
git push origin fix/<short-name>
# then open the PR on github.com
```

For a fix that already lives in ONE isolated commit of ours, `git cherry-pick <sha>` onto the
clean branch. For a fix bundled inside a big release commit, hand-extract just its hunks.

---

## The clean wins (analyzed individually)

Four solid, standalone fixes. Two are bug fixes against issues upstream already has open.

### 1. Water-wedge fix (clip open water rings instead of chord-closing them)

- **What:** in `src/clipping.rs`, `clip_water_ring_to_bbox` now rejects a ring that is **not a
  closed loop** before the "ensure closed" step. Today an open fragment (a multipolygon
  relation whose member way was not loaded, e.g. clipped at the bbox) gets implicitly closed
  with a straight **chord**, and the downstream closure guards accept it and flood a
  triangular/rectangular **water wedge** across correct land. The guard drops the open fragment
  (its sibling closed rings still render, so you get partial water, never a wedge).
- **Why upstream wants it:** this is a **general correctness bug**. Any time a water relation is
  clipped by the render bbox or a member is missing, Arnis can paint a fake water triangle. Not
  Meld-specific.
- **Files:** `src/clipping.rs` only (~20 lines added).
- **Isolation:** clean code, but it lives **inside** the big `fb53c17` (2.9.1) release commit, so
  cherry-pick won't work; hand-extract the one hunk. Upstream does **not** have it (confirmed).
- **Before PR:** reword the comment, it currently name-drops `--osm-tile-dir` (our feature). Make
  it general ("an open fragment from a clipped/partial relation"). Add a small unit test that
  feeds an open ring and asserts `None`.
- **Risk:** very low. Pure guard, no behavior change for already-closed rings.
- **PR title:** `fix(water): drop open (unclosed) water rings instead of chord-closing into a wedge`
- **PR body:**
  > Water multipolygon relations whose member ways are clipped by the render bbox (or otherwise
  > not loaded) can reach `clip_water_ring_to_bbox` as an **open** fragment. The "ensure closed"
  > step then connects last->first with a straight chord, and the closure guards accept the result
  > and flood a triangular water "wedge" across land near the bbox edge. This rejects rings that
  > are not already closed (same end node, or endpoints within one block) before clipping, so a
  > broken fragment is skipped while its sibling closed rings still render. Adds a unit test.

### 2. `--overpass-url` (override the Overpass endpoint)

- **What:** a new flag to point OSM fetching at a custom/self-hosted Overpass instance (or a
  comma-separated list) instead of the public mirror pool.
- **Why upstream wants it:** lots of users get **rate-limited** by the public Overpass; a
  self-hosted or LAN mirror is the standard fix. Small, broadly requested quality-of-life flag.
- **Files:** `src/args.rs` (the flag), `src/retrieve_data.rs` (`fetch_data_from_overpass` takes
  the override list), `src/main.rs` (plumb it through). Optional: `src/gui.rs` to expose it.
- **Isolation:** small and self-contained logically, but bundled in the big `b85c83c` (2.9.0)
  release commit; hand-extract the overpass hunks. Upstream does **not** have it (confirmed).
- **Before PR:** keep it CLI-only for the first PR (skip the GUI field) to stay small; offer the
  GUI toggle as a follow-up. Document the comma-list behavior in `--help`.
- **Risk:** low. Empty by default => exact current behavior.
- **PR title:** `feat(osm): add --overpass-url to use a custom/self-hosted Overpass endpoint`
- **PR body:**
  > The public Overpass mirrors rate-limit per IP, which stalls large or repeated runs. This adds
  > `--overpass-url <url[,url...]>` to send OSM queries to a self-hosted or LAN Overpass instead of
  > the public pool. Empty (default) keeps the current mirror-pool behavior unchanged.

### 3. Datapack Minecraft version range (#1061)

- **What:** widen the tall-world datapack's `pack.mcmeta` `pack_format`/version range and add
  schema overlays so the generated world loads on **MC 1.21.4 through 26.1.x**, not just the one
  version it was pinned to.
- **Why upstream wants it:** directly fixes **their open issue #1061** (worlds failing to load on
  newer Minecraft). High-value, low-risk, asset-level.
- **Files:** `assets/minecraft/datapack_tall/pack.mcmeta`, two `dimension_type/overworld.json`
  overlays, and `src/world_utils.rs` (selects the overlay). Spread across `a617d07`, `734d8de`,
  `bea1c4c`.
- **Isolation:** these three commits are isolated to the datapack feature -> **cherry-pickable**
  as a group onto a clean branch.
- **Before PR:** squash the three into one; confirm the exact `pack_format` numbers match the MC
  versions upstream wants to support (ask in the issue).
- **Risk:** low-medium. Touches world load compatibility; test-load on a couple of MC versions.
- **PR title:** `fix(datapack): support MC 1.21.4 through 26.1.x via pack.mcmeta range + overlays (#1061)`
- **Recipe:**
  ```bash
  git checkout -b fix/datapack-version-range upstream/main
  git cherry-pick a617d07 734d8de bea1c4c   # then squash in the PR
  ```

### 4. Disk-space probe false-block (#824)

- **What:** the "not enough disk space" pre-check could read **0 bytes free** on Windows when the
  path did not exist yet, falsely blocking generation. Now it probes the **nearest existing
  ancestor** directory and only blocks on a **confident positive** reading (an error or a
  0/undeterminable result is treated as "can't tell -> proceed").
- **Why upstream wants it:** fixes **their open issue #824**. People were blocked from generating
  with plenty of disk free. Clean, contained.
- **Files:** `src/gui.rs` only (~17 lines). Related polish in `de5773d` (CWD fallback) and
  `32e690d` (rustfmt).
- **Isolation:** `7f9fcbe` is a clean single-file commit -> **cherry-pickable** (pull `de5773d`
  too).
- **Risk:** low. Strictly loosens a false block; never blocks more than before.
- **PR title:** `fix(gui): don't false-block generation on a 0-byte/undeterminable disk reading (#824)`
- **Recipe:**
  ```bash
  git checkout -b fix/disk-probe-824 upstream/main
  git cherry-pick 7f9fcbe de5773d
  ```

### 5. `--no-buildings` (roads-and-ground only, and skip the Overture fetch)

- **What:** a toggle that keeps buildings **on by default** but lets you turn them off for a
  roads + land-cover + terrain world. Crucially, with buildings off it **skips the supplementary
  Overture building fetch**, which is the dominant per-run cost (~93% of a roads-only run's wall
  time, measured). `main.rs` gates that fetch on `args.buildings`.
- **Why upstream wants it:** lots of people want **just roads and the ground**, no buildings, and
  today they pay the full slow Overture download for nothing. This is both a feature (a clean
  roads-only world) **and** a big speedup for that use case. Buildings default on => zero change
  for everyone else.
- **Flag (ours):**
  ```rust
  /// Skip OSM buildings. Keeps roads, bridges, railways, land cover, water, natural, terrain.
  #[arg(long = "no-buildings", visible_alias = "no-structures",
        default_value_t = true, action = ArgAction::SetFalse)]
  pub buildings: bool,
  ```
- **Files:** `src/args.rs` (flag), `src/main.rs` (gate the Overture fetch on `args.buildings`),
  `src/data_processing.rs` (skip building placement when off).
- **Isolation:** the **core** (skip building footprints + skip Overture fetch) is small and clean.
  Note: our fork *also* strips `man_made` / `power` / `barrier` / pyramids when buildings are off —
  that is **our opinion**, not obviously what upstream wants. **Keep the first PR minimal**:
  `--no-buildings` = no building placement + no Overture fetch. Offer the extra strips as a
  follow-up if asked. Upstream does **not** have this flag (confirmed).
- **Risk:** low. Default-on preserves current behavior exactly.
- **PR title:** `feat: add --no-buildings for a roads-and-ground world (skips the Overture fetch)`
- **PR body:**
  > Adds `--no-buildings` (default off, i.e. buildings stay on) for users who want roads, land
  > cover, water and terrain without buildings. When set, Arnis skips both building placement and
  > the supplementary Overture building fetch, which dominates runtime, so a roads-only world is
  > also much faster to generate. Default keeps today's behavior unchanged.

### 6. GUI: a "Ground Height (Y)" field (expose the existing `--ground-level` in the UI)

- **What:** Arnis already has `--ground-level` in the **CLI**, but the **Tauri GUI has no control
  for it** (confirmed: upstream `index.html` has no ground-level input). Add a number field so
  GUI users can set it without the command line. We already built this (`6caa8d2`): a
  `Ground Height (Y)` input (min -62, max 319, default -62) wired through `main.js` to the flag.
- **Why upstream wants it:** most users use the GUI, not the CLI. A flag that only exists on the
  CLI is invisible to them. This makes an existing feature usable. Pure UI plumbing, no engine
  change.
- **Files:** `src/gui/index.html` (the field), `src/gui/js/main.js` (read it, pass `--ground-level`),
  `src/gui.rs` (accept it). Small.
- **Isolation:** clean, but bundled in `6caa8d2` (which also adds road-detail + seed GUI fields).
  **Extract just the ground-height field** for this PR; road-detail/seed are separate features.
- **Risk:** very low. New optional field, default = current behavior.
- **PR title:** `feat(gui): add a Ground Height (Y) field for the existing --ground-level flag`
- **PR body:**
  > `--ground-level` exists on the CLI but has no GUI control, so GUI users cannot change where the
  > ground surface sits. This adds a numeric "Ground Height (Y)" field (default -62) that passes
  > `--ground-level` through. No engine change; default keeps current behavior.

### Already in upstream / no PR needed

- **`--ground-level` (editable world floor):** **already exists in Arnis** —
  `#[arg(long, default_value_t = -62)] pub ground_level: i32`. So the "start the world higher than
  the lowest point, keep Arnis's default but editable" idea is **already done**: just pass
  `--ground-level <n>` (default `-62`). We did not change it. No PR.
- **Tree apex-leaf cap:** our `f55608f` was cherry-picked **from** upstream (`e545ad1`, an ancestor
  of `upstream/main`); upstream's `tree.rs` already has it. Not ours to PR.

### Bonus clean win found in the deeper analysis

- **Skip buildings sitting in open water** (`src/element_processing/buildings.rs`): don't place a
  building whose footprint is >=60% water, so houses don't end up floating/submerged after the
  water carve. Small, standalone, general bug fix. Good additional PR.
  Title: `fix(buildings): skip buildings whose footprint is mostly open water`.

---

## Notes on the rest (not in the first batch)

Bigger or more opinionated. Each becomes its own small PR(s) later; do **not** bundle.

- **Overture: gate on buildings + on-disk cache** (`src/overture.rs`, `--no-buildings`,
  prewarm). The gate alone is a *major* perf win for everyone: the supplementary Overture
  building fetch was ~93% of a roads-only cell's time and ran even with buildings off. Two PRs:
  (a) skip the fetch when buildings are disabled; (b) cache the STAC index + partition files on
  disk (lock + atomic) so repeat runs do not re-download. High value, medium size.

- **Elevation robustness** (`src/elevation/providers/aws_terrain.rs`): atomic terrain-tile cache
  writes + retries + jitter + retry-missing-tiles, and **fail-fast on stalled fetches**
  (`ae13ac1`). Fixes dark-band / flat-region seams when many tiles fetch at once and helps any
  user on a flaky connection. Slice: the fail-fast is a tiny standalone PR; the atomic-cache +
  retry is a second one.

- **`--download-only` / `--download-terrain-only`** (warm OSM / AWS and exit): clean offline / CI /
  airgapped workflow flags. Small, standalone, but explain the use case in the PR.

- **Seamless tiling core** (`--master-origin-lat/lng`, `--tile-invariant-rendering` / `--seed`,
  and the mpd_lon-anchored-at-origin coordinate fix): lets anyone render a large area in pieces
  that line up (deterministic palette + absolute-origin output + no seam drift). Frame it as
  "render large areas in tiles." The `--seed` determinism is the smallest standalone slice; the
  origin + coord fix is a second, bigger PR. Medium effort, needs a clear write-up.

- **`--osm-tile-dir`** (read pre-split z11 grid tiles directly, dedup by `(type,id)`): nicher
  (built for Meld's shared cache), but the slippy-tile math + dedup are clean. Lower priority for
  upstream unless they want a tiled-input path.

- **Road-detail modes** (auto/max/clean/compact): opinionated rendering control, useful at small
  scales. One self-contained PR, but expect bikeshedding on the mode names/behavior.

- **Water rework** (`src/water_depth.rs`, `src/ground_generation.rs`, ~600 lines): heavily
  Meld-tuned shore/wetland/depth work. **Do not** submit as one blob. Pull out at most one or two
  *defensible, isolated quality fixes* (e.g. a single shore/bed correctness fix) and leave the
  rest in the fork. Highest chance of rejection / endless review.

- **Not upstreamable:** the GUI Meld rebrand (logo, naming, CSS), and anything framed around
  Meld's orchestrator.

---

## Suggested order

1. #4 disk-probe (#824) and #3 datapack (#1061) first: they fix *upstream's own open issues* and
   are the most likely instant merges.
2. #1 water-wedge (real bug, tiny) and #2 `--overpass-url` (popular QoL).
3. Then the Overture gate, then the elevation fail-fast.
4. Larger items (Overture cache, seam core, road-detail) once you have a track record with them.

Each PR: branch off `upstream/main`, one fix, green CI, clear "what + why + Fixes #NNN" body.
