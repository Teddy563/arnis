use crate::coordinate_system::geographic::LLBBox;
use clap::{ArgAction, Parser};
use std::path::PathBuf;
use std::time::Duration;

/// Command-line arguments parser
#[derive(Parser, Debug)]
#[command(author, version, about)]
pub struct Args {
    /// Bounding box of the area (min_lat,min_lng,max_lat,max_lng) (required)
    #[arg(long, allow_hyphen_values = true, value_parser = LLBBox::from_str)]
    pub bbox: LLBBox,

    /// JSON file containing OSM data (optional)
    #[arg(long, group = "location")]
    pub file: Option<String>,

    /// Directory of OSM grid tiles (osm_g1_z{Z}_{X}_{Y}.json) to read the cell's tiles from DIRECTLY
    /// instead of one merged --file. Meld fills this dir; Arnis computes the tiles overlapping --bbox,
    /// loads + dedups them — so there is NO per-cell merge step. Mutually exclusive with --file.
    #[arg(long = "osm-tile-dir", group = "location")]
    pub osm_tile_dir: Option<String>,

    /// Zoom of the OSM grid tiles in --osm-tile-dir (Meld's OSM_GRID_Z; default 11).
    #[arg(long = "osm-tile-z", default_value_t = 11)]
    pub osm_tile_z: u8,

    /// JSON file to save OSM data to (optional)
    #[arg(long, group = "location")]
    pub save_json_file: Option<String>,

    /// Output directory for the generated world (required for Java, optional for Bedrock).
    /// Use --output-dir (or the deprecated --path alias) to specify where the world is created.
    #[arg(long = "output-dir", alias = "path")]
    pub path: Option<PathBuf>,

    /// Directory of tree schematic packs (.schem, grouped by species sub-folder) to
    /// stamp instead of procedural trees. Optional; unset keeps procedural trees.
    /// Lets anyone drop in their own tree pack later without a rebuild.
    #[arg(long = "tree-pack", value_name = "DIR")]
    pub tree_pack: Option<PathBuf>,

    /// Enabled tree size tiers for a region pack, comma-separated: small,medium,big,tall,giant.
    /// Unset = small,medium,big,tall (Giant off). The Giant tier (29+ blocks) only renders at 1:1
    /// even when enabled. A disabled tier falls back to a smaller one (never leaves a gap).
    #[arg(long = "tree-sizes", value_name = "LIST")]
    pub tree_sizes: Option<String>,

    /// Relative tree-size popularity for a region pack, as name=percent pairs
    /// (small,medium,big,tall,giant). 100 = the pack's default share for that tier, 0 = off,
    /// 200 = ~double. Omitted small/medium/big/tall stay 100; giant stays 0 (off, like the old
    /// checkbox). Reweights the same seam-safe size roll; omitting the flag reproduces the default
    /// distribution byte-for-byte. Giant only renders at 1:1 and tiny maps never place tall/giant
    /// regardless of the slider (the scale gate always applies). Wins over --tree-sizes if both set.
    #[arg(long = "tree-size-weights", value_name = "LIST")]
    pub tree_size_weights: Option<String>,

    /// Vertical exaggeration for terrain: multiplies the elevation->Y mapping ONLY (not the
    /// horizontal footprint). 1.0 = true scale; 2-3 = dramatic mountains at the same map size.
    /// Auto-compresses if it would exceed the build height.
    #[arg(long = "vertical-exaggeration", default_value_t = 1.0)]
    pub vertical_exaggeration: f64,

    /// Snow placement mode: off | realistic (latitude snow line) | peaks (top N% of the world's
    /// height range) | manual (at/above a fixed Y).
    #[arg(long = "snow-mode", default_value = "realistic")]
    pub snow_mode: String,

    /// For --snow-mode peaks: snow on the top N% of the world's height range.
    #[arg(long = "snow-percent", default_value_t = 6.0)]
    pub snow_percent: f64,

    /// For --snow-mode manual: snow at/above this Minecraft Y.
    #[arg(long = "snow-y", default_value_t = 80)]
    pub snow_y: i32,

    /// Generate a Bedrock Edition world (.mcworld) instead of Java Edition
    #[arg(long)]
    pub bedrock: bool,

    /// Generate a Luanti/Minetest world (map.sqlite) instead of Java Edition
    #[arg(long)]
    pub luanti: bool,

    /// Evaluate the cave density field on a GPU (EXPERIMENTAL, approximate).
    ///
    /// `off` (default) = CPU. `auto` = best adapter, preferring discrete. `dgpu` /
    /// `igpu` = force that class. Anything else = adapter-name substring. When no
    /// adapter matches, generation falls back to the CPU with a single warning.
    /// GPU results are f32 and may shift the odd cave wall by a block versus the
    /// CPU; the golden-hash gate always runs the CPU path.
    #[arg(long = "gpu", default_value = "off")]
    pub gpu: String,

    /// Generate into a VOID world: only the generated content exists, everything else
    /// is empty air, in this world and in every chunk the game generates later.
    ///
    /// Writes Minecraft's own `the_void` superflat preset into level.dat, so unvisited
    /// chunks stay empty instead of becoming a grass plane, and skips the ground layer
    /// and the base-chunk fill. Server AND singleplayer.
    #[arg(long = "void")]
    pub void_world: bool,

    /// Name shown in the world list. Java worlds otherwise inherit the folder name.
    ///
    /// This sets the name INSIDE level.dat only; it never changes the output
    /// directory, because callers (Meld) read the world back from the exact path
    /// they passed in `--output-dir`.
    #[arg(long = "level-name")]
    pub level_name: Option<String>,

    /// Container for Java region files. `blinear` writes Leaf's B_Linear v3
    /// (`r.X.Z.b_linear`), readable ONLY by Leaf 1.21.11 (June 2026 builds) and newer
    /// and by all 26.x — not by Paper, older Leaf, or the vanilla client.
    #[arg(long = "region-format", value_enum, default_value_t = RegionFormatArg::Mca)]
    pub region_format: RegionFormatArg,

    /// zstd level for `--region-format blinear` buckets. Leaf's own default is 6.
    #[arg(long = "blinear-level", default_value_t = 6, value_parser = clap::value_parser!(i32).range(1..=22))]
    pub blinear_level: i32,

    /// Downloader method (requests/curl/wget) (optional)
    #[arg(long, default_value = "requests")]
    pub downloader: String,

    /// World scale to use, in blocks per meter (1.0 = real size)
    #[arg(long, default_value_t = 1.0, allow_hyphen_values = true, value_parser = parse_scale)]
    pub scale: f64,

    /// Projection mode for coordinate mapping
    /// local: each generation starts at Minecraft (0,0) (default)
    /// web_mercator: global projection for multi-generation worlds
    #[arg(long, default_value = "local")]
    pub projection: crate::projection::ProjectionKind,

    /// Ground level to use in the Minecraft world
    #[arg(long, default_value_t = -62)]
    pub ground_level: i32,

    /// What to generate, mirroring the generation-mode dropdown:
    /// geo-terrain: OSM objects on real elevation terrain
    /// geo-only: OSM objects on flat ground
    /// terrain-only: real elevation terrain, no OSM or Overture objects (--overture has no effect)
    ///
    /// UNSET is NOT geo-terrain in this fork: it falls back to the legacy `--terrain` switch,
    /// so a command with neither flag renders flat ground exactly as it always has.
    #[arg(long, value_enum)]
    pub mode: Option<GenerationMode>,

    /// Legacy terrain switch. Every archived Meld command and every stored Meld project selects
    /// flat vs. real elevation by emitting or omitting this flag, so it stays fully supported:
    /// `--terrain` == `--mode geo-terrain`, omitting it == `--mode geo-only`. Hidden from --help;
    /// `--mode` is the documented spelling.
    #[arg(long = "terrain", hide = true)]
    pub legacy_terrain: bool,

    /// Enable interior generation (optional)
    #[arg(long, default_value_t = true, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub interior: bool,

    /// Enable roof generation (optional)
    #[arg(long, default_value_t = true, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub roof: bool,

    /// Enable filling ground (optional)
    #[arg(long, default_value_t = false)]
    pub fillground: bool,

    /// Only write region files inside this inclusive region rectangle: `rx0,rx1,rz0,rz1`.
    ///
    /// A tiled caller (Meld) renders each cell with a seam-expanded bbox so elements that
    /// straddle the boundary still place their overhang, which makes arnis touch a ring of
    /// halo regions around the cell. The caller then keeps only the cell's canonical
    /// rectangle and deletes the ring - measured at 20 of 36 region files for a 4x4 cell.
    /// Passing the rectangle here skips writing (and compressing, and serialising) files
    /// that are deleted moments later.
    ///
    /// Placement is untouched: the halo is still GENERATED, so blocks that spill across the
    /// seam are still authored into the neighbour's territory exactly as before - the
    /// neighbouring cell renders that ground itself. Only the WRITE is skipped. Do not pass
    /// this for a standalone render: it would drop real terrain nobody else generates.
    // allow_hyphen_values: a rectangle west or north of the origin starts with a minus, and
    // clap otherwise reads "-4,-1,0,3" as an unknown flag and rejects the run. Meld emits the
    // --flag=VALUE form as well; either alone is enough, both is deliberate.
    #[arg(
        long,
        value_name = "RX0,RX1,RZ0,RZ1",
        allow_hyphen_values = true,
        value_parser = parse_region_rect
    )]
    pub canonical_regions: Option<(i32, i32, i32, i32)>,

    /// Underground cave generation (clean-room port of Minecraft 1.21.8 cave worldgen: noise caves +
    /// random-walk carvers + pools/rivers + themed biome zones + ores + decoration + asset-pack
    /// formations). Implies --fillground (caves need solid host rock to carve into).
    /// `--vanilla-caves` is accepted as a legacy alias.
    #[arg(long, alias = "vanilla-caves", default_value_t = false)]
    pub caves: bool,

    /// OPTIONAL directory holding the cave ASSET pack (cave_pack.json + renamed .schem formations -
    /// ice spikes, dripstone columns, amethyst clusters...) stamped into --caves. When omitted,
    /// a `cave-pack/` directory next to the arnis executable is used if present; otherwise the
    /// formation pass is skipped (procedural decoration still runs).
    #[arg(long = "cave-asset-pack", value_name = "DIR")]
    pub cave_asset_pack: Option<std::path::PathBuf>,

    /// Per-biome cave theme amounts as `name=percent` pairs (comma separated). Names: lush,
    /// dripstone, deepdark, mushroom, ice, amethyst, volcanic, coral. 100 = the default share,
    /// 0 = biome off, 200 = roughly double its area (the percent shifts the biome's noise
    /// threshold on a log2 curve, so growth stays smooth). Omitted names stay at 100; omitting
    /// the flag entirely reproduces the default distribution byte-for-byte. Depth/terrain gates
    /// (volcanic bottom-only, ice under mountains only, coral in water pools) always apply.
    /// Example: --cave-biomes lush=150,deepdark=0,amethyst=60
    #[arg(long = "cave-biomes", value_name = "LIST")]
    pub cave_biomes: Option<String>,

    /// Render the cave BIOME ZONE layout for --bbox and exit (no world generation): writes
    /// `<PREFIX>-upper.png` (upper caves, y=-20) and `<PREFIX>-deep.png` (deep caves, y=-48),
    /// transparent where the cave is plain rock, one color per theme, plus a JSON line with the
    /// measured share of each theme on stdout. Honours --seed, --scale, --master-origin and
    /// --cave-biomes, so the preview matches what --caves will carve for the same world.
    #[arg(long = "cave-zone-map", value_name = "PREFIX")]
    pub cave_zone_map: Option<std::path::PathBuf>,

    /// Sample step for --cave-zone-map, in blocks per output pixel (one zone sample per
    /// STEP×STEP square — bigger = chunkier squares, like a noise-cell view; 512 = one
    /// sample per region). Omitted = automatic fine sampling (image capped at 1536px).
    #[arg(long = "cave-zone-map-step", value_name = "BLOCKS")]
    pub cave_zone_map_step: Option<u32>,

    /// Render the Koppen CLIMATE layout for --bbox to `<PREFIX>.png` and exit, without creating a
    /// world. One color per grouped climate (the same grouping that drives biome tint and
    /// arid/polar surface blocks), plus a JSON line with the measured share of each group on
    /// stdout (prefix `CLIMATEMAP `). Sampled in lat/lon over --bbox, so it is a pure function of
    /// the bounding box and lines up with a geographic map overlay.
    #[arg(long = "climate-map", value_name = "PREFIX")]
    pub climate_map: Option<std::path::PathBuf>,

    /// Render the elevation heightmap for --bbox to `<PREFIX>.png` and exit, without creating a
    /// world. Uses the REAL provider stack generation uses (Mapterhorn / regional / AWS), so the
    /// preview matches the terrain the world will get. Prints an `ELEVMAP {json}` line with min/max
    /// metres + bbox for a geographic map overlay.
    #[arg(long = "elevation-map", value_name = "PREFIX")]
    pub elevation_map: Option<std::path::PathBuf>,

    /// Shading for --elevation-map: `hillshade` (default) or `grayscale`.
    #[arg(long = "elevation-map-mode", value_name = "MODE")]
    pub elevation_map_mode: Option<String>,

    /// Enable land cover classification (optional)
    /// When enabled, fetches ESA WorldCover satellite data to classify terrain
    /// (forests, deserts, wetlands, built-up areas, etc.) and select appropriate
    /// surface blocks. Requires --terrain to be enabled.
    #[arg(long = "land-cover", alias = "city-boundaries", default_value_t = true, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub land_cover: bool,

    /// Disable fetching 3D models from external sources (3DMR + Wikimedia).
    #[arg(long = "no-3d", default_value_t = true, action = ArgAction::SetFalse)]
    pub use_3d: bool,

    /// Skip OSM buildings. Keeps everything else (roads, bridges, railways, land cover,
    /// water, natural, terrain) - useful for a roads-and-ground-only world.
    #[arg(long = "no-buildings", visible_alias = "no-structures", default_value_t = true, action = ArgAction::SetFalse)]
    pub buildings: bool,

    /// Add building footprints from Overture Maps that are missing in OpenStreetMap.
    /// Helps sparsely mapped areas; may occasionally add a satellite-detected false positive.
    ///
    /// Defaults to true, which is exactly what the fork did before this flag existed: Overture
    /// was fetched unconditionally whenever buildings were on, with no way to separate the two.
    /// The only way to avoid a false-positive building was --no-buildings, which also throws
    /// away every real one. Keeping the default at true means no archived command, saved Meld
    /// project or cached render changes output by one block.
    #[arg(long = "overture", default_value_t = true, action = ArgAction::Set, num_args = 0..=1, default_missing_value = "true")]
    pub overture: bool,

    /// Pre-warm the Overture building cache for --bbox (fetch + cache the byte ranges) and exit,
    /// before any world creation. Lets Meld download a region's Overture data once, up front, so the
    /// later parallel cells read it from disk instead of each stalling on a cold fetch.
    #[arg(long = "prewarm-overture", default_value_t = false)]
    pub prewarm_overture: bool,

    /// Pre-warm the elevation TILE cache for --bbox (Mapterhorn terrarium tiles, or the regional/
    /// AWS provider generation would pick) and exit, before any world creation. Fills the exact
    /// per-tile disk cache the generation cells read, so later parallel cells hit disk instead of
    /// rate-limiting the tile server. Companion to --prewarm-overture (buildings). Purely additive.
    #[arg(long = "prewarm-elevation", default_value_t = false)]
    pub prewarm_elevation: bool,

    /// Enable debug mode (optional)
    #[arg(long)]
    pub debug: bool,

    /// Set floodfill timeout (seconds) (optional)
    ///
    /// Guards against a pathological polygon hanging a run: once the budget is spent, the
    /// fill stops seeding new flood fronts and returns what it has. Omitted = no limit.
    ///
    /// TWO MEANINGS, selected by the environment, because a wall clock is not deterministic.
    /// By DEFAULT this is elapsed time measured with `Instant::now()`, so a fill under heavy
    /// machine load can truncate at a different point than the same fill on an idle box —
    /// the same bbox rendered twice can differ, and two adjacent tiles rendered by different
    /// workers can disagree along their border. With `ARNIS_FILL_BUDGET=1` the seconds are
    /// converted, through one documented constant in `floodfill.rs`
    /// (`BUDGET_UNITS_PER_SECOND`), into a work-unit budget counted from the fill's own
    /// iterations, so the stopping point is a pure function of the input. The flag keeps its
    /// meaning either way; only what is being counted changes.
    ///
    /// The env var is OFF unless set: an unset run behaves exactly as it always has.
    #[arg(long, value_parser = parse_duration)]
    pub timeout: Option<Duration>,

    /// Spawn point latitude (optional, must be within bbox)
    #[arg(long, allow_hyphen_values = true)]
    pub spawn_lat: Option<f64>,

    /// Spawn point longitude (optional, must be within bbox)
    #[arg(long, allow_hyphen_values = true)]
    pub spawn_lng: Option<f64>,

    /// Clockwise rotation angle in degrees (optional, range: -90 to 90)
    #[arg(long, default_value_t = 0.0, allow_hyphen_values = true)]
    pub rotation: f64,

    /// Master origin latitude for global coordinates (optional)
    #[arg(long, allow_hyphen_values = true)]
    pub master_origin_lat: Option<f64>,

    /// Master origin longitude for global coordinates (optional)
    #[arg(long, allow_hyphen_values = true)]
    pub master_origin_lng: Option<f64>,

    /// Global elevation minimum in metres for cross-tile Y normalisation (optional).
    /// When set alongside --elevation-max, all tiles map this real-world elevation to
    /// ground_level, eliminating height seams at cell boundaries.
    #[arg(long, allow_hyphen_values = true)]
    pub elevation_min: Option<f64>,

    /// Global elevation maximum in metres for cross-tile Y normalisation (optional).
    #[arg(long, allow_hyphen_values = true)]
    pub elevation_max: Option<f64>,

    /// Tile-invariant building rendering with optional numeric seed.
    /// When set, building decisions (skyscraper/footprint/diagonality/
    /// start-Y) read pre-clip bounds from `ProcessedWay.unclipped_bounds`
    /// rather than the bbox-clipped node list, AND every salted RNG
    /// stream mixes the supplied seed into its derivation. Combined
    /// with PR 1 + PR 2, makes a building that straddles two adjacent
    /// tiles render with identical block choices in both. Required by
    /// external schedulers (e.g. Meld) that generate adjacent bboxes
    /// into one Minecraft world.
    ///
    /// Forms accepted:
    ///   --tile-invariant-rendering         (treated as seed=1)
    ///   --tile-invariant-rendering 1
    ///   --tile-invariant-rendering 42
    ///   --seed 42                          (alias)
    ///
    /// Off (omitted): upstream behaviour — clipped nodes drive every
    /// decision; salted RNG seeded from element_id + salt only.
    /// On (Some(N)): pre-clip metrics carry through AND every salted
    /// stream is seeded as `element_id ^ salt^32 ^ N^16`, so two runs
    /// passing the same N produce byte-identical building palette.
    #[arg(long = "tile-invariant-rendering",
          visible_alias = "seed",
          num_args = 0..=1, default_missing_value = "1",
          value_parser = clap::value_parser!(u64))]
    pub tile_invariant_rendering: Option<u64>,

    /// Extend build height via a generated pack (Java 1.21.4+: Y=-2032..2031;
    /// Bedrock 1.21.40+: Y=-512..512). Both are experimental.
    ///
    /// The world declares exactly the vertical range its terrain needs — the smallest
    /// legal height, not a fixed 4064 preset — because every extra 16 blocks is another
    /// chunk section per column in lighting, heightmaps and region files.
    #[arg(long, default_value_t = false)]
    pub disable_height_limit: bool,

    /// Target Minecraft version, e.g. `26.1.2`. Decides the DataVersion written into
    /// every chunk, whether extended height may be declared at all (1.17+), and the chunk
    /// layout. Only versions with VERIFIED constants in assets/mc_versions.json are
    /// accepted — an unknown version is refused rather than guessed at, because a wrong
    /// DataVersion yields a world that loads and then quietly misbehaves. Omitted = the
    /// writer's historical default.
    #[arg(long = "mc-version", value_name = "VERSION")]
    pub mc_version: Option<String>,

    /// Blocks kept free above the highest terrain for trees and buildings when fitting an
    /// extended-height world.
    #[arg(long = "height-headroom", default_value_t = 32, value_name = "BLOCKS")]
    pub height_headroom: i32,

    /// Blocks kept free below the lowest terrain for caves and water carving when fitting
    /// an extended-height world.
    #[arg(long = "height-underroom", default_value_t = 16, value_name = "BLOCKS")]
    pub height_underroom: i32,

    /// Blocks of room reserved under the terrain datum for the deepest water carve.
    ///
    /// SEAM-CRITICAL for tiled generation. Left unset, the clearance is MEASURED from
    /// this tile's own land cover, so a tile containing deep water sets its datum higher
    /// than an inland neighbour and the two place identical terrain at different Y — a
    /// Y-cliff along the whole cell border. An orchestrator generating adjacent tiles
    /// (Meld) must pass the SAME value to every tile.
    ///
    /// `max` uses the engine's worst case, which is exact: the carve depth is bounded by
    /// a compile-time constant, so `max` always clears the deepest possible water without
    /// reserving anything speculative. A number sets it explicitly. Unset = measure
    /// per-tile (the historical single-world behaviour).
    #[arg(long = "water-carve-clearance", value_name = "max|BLOCKS")]
    pub water_carve_clearance: Option<String>,

    /// Explicit world floor Y, overriding the fitted one. Must be a multiple of 16 within
    /// -2032..=2031, and is REFUSED (never clamped) if it would cut into the terrain.
    #[arg(long = "min-y", allow_hyphen_values = true, value_name = "Y")]
    pub min_y: Option<i32>,

    /// Explicit world ceiling Y, overriding the fitted one. `min-y + height` may not
    /// exceed 2032, and a ceiling below the terrain's peak is refused rather than
    /// silently shearing the peaks off.
    #[arg(long = "max-y", allow_hyphen_values = true, value_name = "Y")]
    pub max_y: Option<i32>,

    /// Skip the regional high-resolution elevation providers  and only use
    /// AWS Terrain Tiles for faster generation.
    #[arg(long, default_value_t = false)]
    pub aws_only_elevation: bool,

    /// Use ONLY the regional high-resolution elevation providers (USGS, IGN
    /// France/Spain, Japan GSI): never select AWS Terrain Tiles and never fall
    /// back to them when a regional provider fails or returns empty data — the
    /// run errors instead (so an orchestrator can retry) rather than silently
    /// generating from AWS's broken no-data tiles. Mutually exclusive with
    /// --aws-only-elevation.
    #[arg(long, default_value_t = false)]
    pub regional_elevation_only: bool,

    /// Print generation-only timing to stderr (excludes data fetching)
    #[arg(long, hide = true)]
    pub benchmark: bool,

    /// Override the Overpass API endpoint(s) used to fetch OSM data.
    /// Comma-separated list, in priority order. When set, this list is
    /// used INSTEAD of the public Overpass mirror pool — useful for
    /// pointing Arnis at a self-hosted Overpass instance to bypass
    /// public rate limits during large batch generation.
    ///
    /// Example: --overpass-url http://localhost:12345/api/interpreter
    /// Example: --overpass-url http://lan-host:12345/api/interpreter,https://overpass-api.de/api/interpreter
    ///
    /// Upstream-friendly improvement: this is purely additive — when
    /// the flag is omitted, Arnis behaves exactly as before. Safe to
    /// upstream for users running batch jobs against private mirrors.
    #[arg(long = "overpass-url", value_delimiter = ',')]
    pub overpass_url: Vec<String>,

    /// Road rendering detail level. Controls how OSM highway features
    /// are simplified at low scale (block resolution > 1 m) where
    /// footways/crosswalks/lane-dividers/multi-lane widths stack onto
    /// the same blocks and produce checker noise at intersections.
    ///
    ///   max     (default) — render every highway exactly as upstream
    ///                       Arnis does.
    ///   clean             — visual-cleanup pass for scale ≥ 0.7.
    ///   compact           — vehicle-grade highways only, lanes capped.
    ///
    /// Upstream-friendly: when omitted, behaviour identical to upstream
    /// (max). Multi-tile schedulers (Meld) auto-select `compact` at
    /// scale<0.7 and `clean` at scale≥0.7.
    #[arg(long = "road-detail", default_value = "max",
          value_parser = ["max", "clean", "compact"])]
    pub road_detail: String,

    /// River bed profile.
    ///
    ///   off  (default) — legacy terraced bed; byte-identical output.
    ///   v1             — OSM-tagged rivers (waterway=river/stream/canal
    ///                    ways, water=river polygons, riverbanks) get a
    ///                    smooth width-scaled U profile: soft banks, rounded
    ///                    bottom, no dunes or bank wobble.
    ///
    /// RIVERS ONLY — lake and coastline beds are untouched, and a river
    /// blends into them instead of stepping. Versioned: any future retune of
    /// the profile tables ships as `v2`, never as a changed `v1`, so a world
    /// generated with `v1` is reproducible forever.
    #[arg(long = "river-bed", default_value = "off",
          value_parser = ["off", "v1"])]
    pub river_bed: String,

    /// Road longitudinal grading.
    ///
    ///   off  (default) — legacy cross-section-only flattening;
    ///                    byte-identical output.
    ///   on             — each road is graded along its length with a
    ///                    slope-limited profile (per-class max grade), so
    ///                    surfaces climb in evenly spaced ramps instead of
    ///                    following terrain contour steps. Junction Ys are
    ///                    pinned so crossing roads agree, and road-surface
    ///                    overrides fold by min-Y so results are order-free.
    #[arg(long = "road-grade", default_value = "off",
          value_parser = ["off", "on"])]
    pub road_grade: String,

    /// Bake per-chunk lighting so distant chunks render lit in LOD mods
    /// (Voxy/Chunky) without visiting them. Slower; off by default.
    #[arg(long, default_value_t = false)]
    pub bake_lighting: bool,

    /// Download OSM data to --save-json-file and exit, skipping world generation.
    /// Lets an external scheduler (Meld) pre-fetch a whole region's OSM in ONE
    /// request and feed it to many cells via --file, instead of each parallel cell
    /// querying Overpass (which trips the public per-IP rate limit). Requires
    /// --save-json-file. Purely additive: when omitted, behaviour is unchanged.
    #[arg(long = "download-only", default_value_t = false)]
    pub download_only: bool,

    /// Download/warm the AWS terrain (elevation) tiles for --bbox into the shared tile
    /// cache and exit, skipping world generation. Lets an external scheduler (Meld)
    /// pre-warm a whole region's elevation tiles in ONE single-process pass (8 concurrent
    /// requests) instead of many parallel cell processes hammering S3 at once (which
    /// rate-limits and returns truncated tiles -> flat terrain seams). Companion to
    /// --download-only (OSM). Purely additive: when omitted, behaviour is unchanged.
    #[arg(long = "download-terrain-only", default_value_t = false)]
    pub download_terrain_only: bool,

    /// Hard offline mode for elevation: never hit the network for terrain tiles
    /// or regional elevation providers. Cache hits are served as usual; a cache
    /// MISS returns an error, and Arnis's existing fallbacks turn that into
    /// flat/NaN ground. Sets the ARNIS_OFFLINE env var for the elevation
    /// providers. Purely additive: when omitted, behaviour is unchanged.
    #[arg(
        long = "offline",
        visible_alias = "elevation-cache-only",
        default_value_t = false
    )]
    pub offline: bool,

    /// Custom chest loot table (JSON) for building interiors. When omitted, the
    /// built-in default table is used. A malformed file logs a warning and falls
    /// back to the default (never fails a run). Get the default's shape with
    /// --dump-loot-table. Purely additive: when omitted, behaviour is unchanged.
    #[arg(long = "loot-table")]
    pub loot_table: Option<std::path::PathBuf>,

    /// Write the built-in default loot table to the given path as JSON and exit.
    /// Used by Meld's web loot editor as the single source of truth for its
    /// default/reset. Hidden helper; no world is created.
    #[arg(long = "dump-loot-table", hide = true)]
    pub dump_loot_table: Option<std::path::PathBuf>,

    /// Which bundled schematic-prop families to place: `all`, `none`, or a comma
    /// list (car, boat, crane, excavator, fountain, helicopter, lighthouse,
    /// playground, starship, tombstone, tractor, windturbine). Default all.
    /// Purely additive: when omitted, behaviour is unchanged (all families on).
    #[arg(long = "props", default_value = "all")]
    pub props: String,

    /// Smallest world scale at which schematic props are placed. Props are fixed-size
    /// builds (a boat is a boat), so on a scaled-down world they keep their block size
    /// while everything around them shrinks — at 1:10 a parked crane is the size of a
    /// district. Below this scale they are skipped. Set 0 to place them at any scale.
    #[arg(long = "props-min-scale", default_value_t = 0.35)]
    pub props_min_scale: f64,

    /// Player game mode written into the generated world's level.dat (Java).
    #[arg(long, value_enum, default_value_t = GameMode::Creative)]
    pub gamemode: GameMode,

    /// Initial time of day in ticks (0 = dawn, 6000 = noon, 18000 = midnight).
    #[arg(long, default_value_t = 6000, value_parser = clap::value_parser!(i64).range(0..24000))]
    pub world_time: i64,

    /// Place a locked filled-map of the whole world in the player's inventory (Java only).
    #[arg(long = "map-item", default_value_t = false)]
    pub map_item: bool,

    /// Add the world map item to the EXISTING Java world at --output-dir and exit, without
    /// generating anything. Derives the map footprint from the saved region files, so an
    /// external scheduler (Meld) can run one post-merge pass over a fully assembled master
    /// world. Requires --output-dir; --bbox is ignored for the map bounds.
    #[arg(long = "map-item-only", default_value_t = false)]
    pub map_item_only: bool,

    /// Farmland texture mix as a `name=pct` list over the five categories
    /// `coarse,plains,flower,farm,moss` (relative shares, any subset/order), e.g.
    /// `plains=60,coarse=20,flower=10,farm=10,moss=15`. Splits OSM farmland into
    /// coherent ~7 m patches. Omitted (or all-zero) = stock all-farmland, byte-identical.
    ///
    /// The two looks, and what they cost. Omitted is the CLASSIC surface: one uniform
    /// sheet of crops, the cheapest run. Any mix turns farmland into separate parcels,
    /// each growing a single crop and divided by dirt tracks; on farmland-heavy areas
    /// that costs roughly 15% more generation time and about 1.5x the peak memory.
    /// Note `farm=100` is a mix like any other: it keeps the farmland surface but still
    /// lays out parcels and tracks, so it is NOT the same as omitting the flag.
    #[arg(long = "field-mix", value_name = "LIST")]
    pub field_mix: Option<String>,

    /// Scatter rock formations (andesite/tuff schematics) on textured fields.
    #[arg(long = "rocks", default_value_t = false)]
    pub rocks: bool,

    /// Rocks per 512×512 region of field area. 0 = none. Only used with --rocks.
    #[arg(long = "rock-density", default_value_t = 4, value_parser = clap::value_parser!(u8).range(0..=64))]
    pub rock_density: u8,

    /// Scatter bushes (foliage schematics) on textured fields.
    #[arg(long = "bushes", default_value_t = false)]
    pub bushes: bool,

    /// Bushes per 512×512 region of field area. 0 = none. Only used with --bushes.
    #[arg(long = "bush-density", default_value_t = 8, value_parser = clap::value_parser!(u8).range(0..=64))]
    pub bush_density: u8,

    /// Also texture OSM grassland (meadow/grass/greenfield/orchard) with the built-in
    /// grass profile (large loose plots), not just farmland. Additive; off = unchanged.
    #[arg(long = "grass-texture", default_value_t = false)]
    pub grass_texture: bool,

    /// Also texture UNTAGGED land from satellite land-cover: ESA cropland gets the
    /// land mix (below), ESA grassland gets the grass profile. Additive; off = unchanged.
    #[arg(long = "land-texture", default_value_t = false)]
    pub land_texture: bool,

    /// Mix for UNTAGGED satellite cropland (same `name=pct` format as --field-mix).
    /// Omitted = reuse --field-mix, so untagged land can have its own shares.
    #[arg(long = "land-mix", value_name = "LIST")]
    pub land_mix: Option<String>,

    /// Farm-plot crop shares as a `name=pct` list over
    /// `wheat,potato,carrot,beetroot,sunflower,pumpkin,fallow`. Each farm parcel grows
    /// ONE crop picked by these weights (real monoculture plots). Omitted = the default
    /// combined patchwork `wheat=40,potato=15,carrot=15,beetroot=8,sunflower=12,pumpkin=5,fallow=5`.
    #[arg(long = "farm-crops", value_name = "LIST")]
    pub farm_crops: Option<String>,

    /// Field-pattern zoom: scales all parcel/plot sizes by this percent (25-400,
    /// 100 = default). Larger = bigger fields.
    #[arg(long = "field-scale", default_value_t = 100, value_parser = clap::value_parser!(u16).range(25..=400))]
    pub field_scale: u16,

    /// Mix for textured GRASSLAND (meadow/grass/orchard + satellite grassland), same
    /// `name=pct` format as --field-mix. Omitted = the built-in grassy blend.
    #[arg(long = "grass-mix", value_name = "LIST")]
    pub grass_mix: Option<String>,
}

/// What a run generates. Same three values as the GUI's `generation-mode-select`
/// (src/gui/js/main.js:1567) and Meld's mode dropdown.
///
/// Deliberately NOT `Default`: upstream defaults this to GeoTerrain (terrain ON), the fork
/// defaults to flat ground. Leaving the trait off makes an accidental `unwrap_or_default()`
/// a compile error rather than a silently different world.
/// Region container for Java worlds, selected by `--region-format`.
///
/// A container swap only: the chunk NBT is identical either way, so the same world can
/// be converted between the two without touching a block.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum RegionFormatArg {
    /// Anvil `r.X.Z.mca` — the universal format every server and the client read.
    #[default]
    Mca,
    /// Leaf B_Linear v3 `r.X.Z.b_linear` — server-only, Leaf 1.21.11+ / 26.x.
    Blinear,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum GenerationMode {
    /// OSM objects on real elevation terrain
    GeoTerrain,
    /// OSM objects on flat ground
    GeoOnly,
    /// Real elevation terrain without any OSM or Overture objects
    TerrainOnly,
}

impl GenerationMode {
    /// Whether real elevation is fetched and applied instead of flat ground.
    #[inline]
    pub fn terrain(self) -> bool {
        !matches!(self, GenerationMode::GeoOnly)
    }

    /// Whether OSM and Overture objects are skipped entirely.
    #[inline]
    pub fn skip_objects(self) -> bool {
        matches!(self, GenerationMode::TerrainOnly)
    }
}

impl Args {
    /// Effective mode: an explicit --mode wins; otherwise the legacy --terrain switch decides.
    /// That fallback is what preserves the fork's flat-ground default for archived commands.
    #[inline]
    pub fn generation_mode(&self) -> GenerationMode {
        self.mode.unwrap_or(if self.legacy_terrain {
            GenerationMode::GeoTerrain
        } else {
            GenerationMode::GeoOnly
        })
    }

    /// Whether this run uses real elevation terrain rather than flat ground.
    #[inline]
    pub fn terrain(&self) -> bool {
        self.generation_mode().terrain()
    }

    /// Whether this run skips OSM/Overture objects (terrain-only).
    #[inline]
    pub fn skip_objects(&self) -> bool {
        self.generation_mode().skip_objects()
    }
}

/// Player game mode for the generated world.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum GameMode {
    Survival,
    Creative,
    Spectator,
}

impl GameMode {
    pub fn java_game_type(self) -> i32 {
        match self {
            GameMode::Survival => 0,
            GameMode::Creative => 1,
            GameMode::Spectator => 3,
        }
    }
}

/// Smallest usable scale. Meld's own scale box floors at 0.01 (1:100), and its planet
/// renders live down there, so this is set to Meld's floor rather than upstream's 0.05.
///
/// MELD-DIVERGENCE: upstream also defines OBJECT_SKIP_SCALE = 0.3 and makes
/// `skip_objects()` true below it, which drops every OSM and Overture object at low
/// scale. That is deliberately NOT taken here. Meld's default project scale is 0.1 and
/// its entire purpose is rendering countries at 1:10 WITH buildings, roads and rails;
/// adopting it would silently empty every default Meld render. The fork already has the
/// right shape of that idea scoped per feature, in `--props-min-scale`.
pub const MIN_SCALE: f64 = 0.01;
/// Largest usable scale. Beyond this a single square kilometre already costs GBs and hours.
pub const MAX_SCALE: f64 = 4.0;

/// Rejects NaN, the infinities and out-of-range scales. Used by both the CLI parser and
/// `validate_args`, so an invalid scale can never reach the (expensive) fetch stage.
pub fn validate_scale(scale: f64) -> Result<(), String> {
    if !scale.is_finite() {
        return Err("World scale must be a finite number.".to_string());
    }
    if !(MIN_SCALE..=MAX_SCALE).contains(&scale) {
        return Err(format!(
            "World scale must be between {MIN_SCALE} and {MAX_SCALE} (got {scale})."
        ));
    }
    Ok(())
}

fn parse_scale(arg: &str) -> Result<f64, String> {
    let scale: f64 = arg
        .parse()
        .map_err(|_| format!("`{arg}` is not a number"))?;
    validate_scale(scale)?;
    Ok(scale)
}

/// Validates CLI arguments after parsing.
/// For Java Edition: `--path` is required. If the directory doesn't exist, it will be created.
/// For Bedrock Edition (`--bedrock`): `--path` is optional (defaults to Desktop output).
pub fn validate_args(args: &Args) -> Result<(), String> {
    // First, and above every early-exit return below: the download / prewarm / map-render
    // modes all still transform coordinates, so a NaN or zero scale has to be refused for
    // them too.
    validate_scale(args.scale)?;

    // --terrain asks for real elevation; --mode geo-only asks for flat ground. Refuse the
    // contradiction rather than silently picking one. MUST stay above every `return Ok(())`
    // early exit below, or the download / prewarm / map-render modes would skip the check.
    if args.legacy_terrain && args.mode == Some(GenerationMode::GeoOnly) {
        return Err(
            "--terrain contradicts --mode geo-only (flat ground). Drop --terrain, or use --mode geo-terrain."
                .to_string(),
        );
    }

    if args.aws_only_elevation && args.regional_elevation_only {
        return Err(
            "--aws-only-elevation and --regional-elevation-only are mutually exclusive."
                .to_string(),
        );
    }
    // Download-only just fetches OSM to a file; no world is created, so the
    // --output-dir requirement (and the rest) does not apply.
    if args.download_only {
        if args.save_json_file.is_none() {
            return Err("--download-only requires --save-json-file <path>.".to_string());
        }
        return Ok(());
    }

    // Terrain-only just warms the elevation tile cache for --bbox; no world is created,
    // and --bbox is always parsed, so nothing else is required.
    if args.download_terrain_only {
        return Ok(());
    }

    // Overture pre-warm just fetches + caches Overture ranges for --bbox; no world is created.
    if args.prewarm_overture {
        return Ok(());
    }

    // Elevation pre-warm just fetches + caches elevation tiles for --bbox; no world is created.
    if args.prewarm_elevation {
        return Ok(());
    }

    // Zone-map mode renders cave-biome PNGs for --bbox; no world is created.
    if args.cave_zone_map.is_some() {
        return Ok(());
    }

    // Climate-map mode renders a Koppen climate PNG for --bbox; no world is created.
    if args.climate_map.is_some() {
        return Ok(());
    }

    // Elevation-map mode renders an elevation PNG for --bbox; no world is created.
    if args.elevation_map.is_some() {
        return Ok(());
    }

    // Dump-loot-table just writes the default loot JSON and exits; no world.
    if args.dump_loot_table.is_some() {
        return Ok(());
    }

    if args.bedrock && args.luanti {
        return Err("Cannot use --bedrock and --luanti together.".to_string());
    }

    // The B_Linear container frames Java chunk NBT; Bedrock and Luanti worlds are not
    // built out of chunk NBT at all, so the combination has no meaning.
    // Caves carve rock out of solid ground, and --caves force-enables --fillground.
    // A void world has no rock, so the combination asks for two opposite worlds.
    if args.void_world && args.caves {
        return Err(
            "--void and --caves are mutually exclusive: caves need solid ground to carve into."
                .to_string(),
        );
    }
    if args.void_world && (args.bedrock || args.luanti) {
        return Err("--void currently applies to Java worlds only.".to_string());
    }

    if args.region_format == RegionFormatArg::Blinear && (args.bedrock || args.luanti) {
        return Err(
            "--region-format blinear only applies to Java worlds; drop --bedrock/--luanti."
                .to_string(),
        );
    }

    if args.bedrock {
        // Bedrock: path is optional; if provided, it must be an existing directory
        if let Some(ref path) = args.path {
            if !path.exists() {
                return Err(format!("Path does not exist: {}", path.display()));
            }
            if !path.is_dir() {
                return Err(format!("Path is not a directory: {}", path.display()));
            }
        }
    } else if args.luanti {
        // Luanti: path optional, defaults to OS Luanti worlds dir
        if let Some(ref path) = args.path {
            if !path.exists() {
                return Err(format!("Path does not exist: {}", path.display()));
            }
            if !path.is_dir() {
                return Err(format!("Path is not a directory: {}", path.display()));
            }
        }
    } else {
        // Java: path is required. If it exists, it must be a directory.
        // If it doesn't exist, create_new_world will create it.
        match &args.path {
            None => {
                return Err(
                    "The --output-dir argument is required for Java Edition. Provide the directory where the world should be created. Use --bedrock for Bedrock Edition output."
                        .to_string(),
                );
            }
            Some(ref path) => {
                if path.exists() && !path.is_dir() {
                    return Err(format!(
                        "Path exists but is not a directory: {}",
                        path.display()
                    ));
                }
                // If path doesn't exist, that's OK - create_new_world will create it
            }
        }
    }

    // Validate spawn point: both or neither must be provided
    match (args.spawn_lat, args.spawn_lng) {
        (Some(_), None) | (None, Some(_)) => {
            return Err("Both --spawn-lat and --spawn-lng must be provided together.".to_string());
        }
        (Some(lat), Some(lng)) => {
            // Validate coordinates are valid lat/lng (rejects NaN, inf, out-of-range)
            use crate::coordinate_system::geographic::LLPoint;
            let llpoint =
                LLPoint::new(lat, lng).map_err(|e| format!("Invalid spawn coordinates: {e}"))?;

            // Validate that spawn point is within the bounding box
            if !args.bbox.contains(&llpoint) {
                return Err(
                    "Spawn point (--spawn-lat, --spawn-lng) must be within the bounding box."
                        .to_string(),
                );
            }
        }
        _ => {}
    }

    // Validate rotation angle range (also rejects NaN and infinity)
    if !args.rotation.is_finite() || args.rotation < -90.0 || args.rotation > 90.0 {
        return Err("Rotation angle must be between -90 and 90 degrees.".to_string());
    }

    Ok(())
}

fn parse_duration(arg: &str) -> Result<std::time::Duration, std::num::ParseIntError> {
    let seconds = arg.parse()?;
    Ok(std::time::Duration::from_secs(seconds))
}

/// Parses `--canonical-regions rx0,rx1,rz0,rz1` (inclusive region coordinates).
fn parse_region_rect(v: &str) -> Result<(i32, i32, i32, i32), String> {
    let parts: Vec<&str> = v.split(',').map(|p| p.trim()).collect();
    if parts.len() != 4 {
        return Err(format!(
            "expected four comma-separated region coordinates rx0,rx1,rz0,rz1, got {:?}",
            v
        ));
    }
    let mut n = [0i32; 4];
    for (i, p) in parts.iter().enumerate() {
        n[i] = p
            .parse::<i32>()
            .map_err(|e| format!("region coordinate {:?} is not an integer: {e}", p))?;
    }
    if n[0] > n[1] || n[2] > n[3] {
        return Err(format!(
            "region rectangle is inverted: rx {}..{}, rz {}..{}",
            n[0], n[1], n[2], n[3]
        ));
    }
    Ok((n[0], n[1], n[2], n[3]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generation_mode() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();
        let base = ["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
        let parse = |extra: &[&str]| {
            let mut cmd: Vec<&str> = base.to_vec();
            cmd.extend_from_slice(extra);
            Args::parse_from(cmd.iter())
        };

        // No flags at all: the fork's flat-ground default survives (upstream would be GeoTerrain).
        let args = parse(&[]);
        assert!(!args.terrain());
        assert!(!args.skip_objects());

        let args = parse(&["--mode", "geo-terrain"]);
        assert!(args.terrain() && !args.skip_objects());
        assert!(validate_args(&args).is_ok());

        let args = parse(&["--mode", "geo-only"]);
        assert!(!args.terrain() && !args.skip_objects());
        assert!(validate_args(&args).is_ok());

        let args = parse(&["--mode", "terrain-only"]);
        assert!(args.terrain() && args.skip_objects());
        assert!(validate_args(&args).is_ok());

        // --terrain alone == geo-terrain; redundant with an agreeing --mode is fine
        assert!(parse(&["--terrain"]).terrain());
        let args = parse(&["--terrain", "--mode", "terrain-only"]);
        assert!(args.terrain() && args.skip_objects());
        assert!(validate_args(&args).is_ok());

        // Contradiction refused, and refused even inside an early-exit mode
        assert!(validate_args(&parse(&["--mode", "geo-only", "--terrain"])).is_err());
        assert!(validate_args(&parse(&[
            "--mode",
            "geo-only",
            "--terrain",
            "--download-terrain-only"
        ]))
        .is_err());

        // Unknown modes are rejected by clap
        let mut cmd: Vec<&str> = base.to_vec();
        cmd.extend_from_slice(&["--mode", "objects"]);
        assert!(Args::try_parse_from(cmd.iter()).is_err());
    }

    #[test]
    fn test_flags() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();

        // The legacy --terrain flag still parses and still means "real elevation"
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--terrain",
            "--debug",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(args.debug);
        assert!(args.legacy_terrain);
        assert!(args.terrain());
        assert!(!args.skip_objects());
        assert_eq!(args.generation_mode(), GenerationMode::GeoTerrain);
        assert!(validate_args(&args).is_ok());

        let cmd = ["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
        let args = Args::parse_from(cmd.iter());
        assert!(!args.debug);
        // FORK DEFAULT, NOT upstream's: no flag at all == flat ground.
        assert!(args.mode.is_none());
        assert!(!args.legacy_terrain);
        assert!(!args.terrain());
        assert!(!args.skip_objects());
        assert!(!args.bedrock);
        assert!(!args.disable_height_limit);
        assert!(!args.bake_lighting);
        // interior, roof, land_cover default to true
        assert!(args.interior);
        assert!(args.roof);
        assert!(args.land_cover);
    }

    #[test]
    fn test_bool_flags_can_be_disabled() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();

        // Test disabling interior/roof/land-cover with =false
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--interior=false",
            "--roof=false",
            "--land-cover=false",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(!args.interior);
        assert!(!args.roof);
        assert!(!args.land_cover);

        // Test enabling with bare flag (no value)
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--interior",
            "--roof",
            "--land-cover",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(args.interior);
        assert!(args.roof);
        assert!(args.land_cover);

        // Test backwards compatibility with old --city-boundaries alias
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--city-boundaries=false",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(!args.land_cover);
    }

    #[test]
    fn test_bedrock_flag() {
        // Bedrock mode doesn't require --output-dir
        let cmd = ["arnis", "--bedrock", "--bbox", "1,2,3,4"];
        let args = Args::parse_from(cmd.iter());
        assert!(args.bedrock);
        assert!(args.path.is_none());
        assert!(validate_args(&args).is_ok());
    }

    #[test]
    fn test_disable_height_limit_flag() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();

        // Default is false
        let cmd = ["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
        let args = Args::parse_from(cmd.iter());
        assert!(!args.disable_height_limit);

        // Flag enables it
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--disable-height-limit",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(args.disable_height_limit);
    }

    #[test]
    fn test_java_requires_path() {
        let cmd = ["arnis", "--bbox", "1,2,3,4"];
        let args = Args::parse_from(cmd.iter());
        assert!(!args.bedrock);
        assert!(args.path.is_none());
        assert!(validate_args(&args).is_err());
    }

    #[test]
    fn test_java_nonexistent_path_is_ok() {
        // Java: nonexistent paths are OK - create_new_world will create them
        let tmp = tempfile::tempdir().unwrap();
        let nonexistent = tmp.path().join("does_not_exist");
        let cmd = [
            "arnis",
            "--output-dir",
            nonexistent.to_str().unwrap(),
            "--bbox",
            "1,2,3,4",
        ];
        let args = Args::parse_from(cmd.iter());
        let result = validate_args(&args);
        assert!(result.is_ok());
    }

    #[test]
    fn test_java_path_exists_but_is_file_fails() {
        // Java: if path exists but is a file, fail
        let tmpfile = tempfile::NamedTempFile::new().unwrap();
        let tmp_path = tmpfile.path().to_str().unwrap();

        let cmd = ["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
        let args = Args::parse_from(cmd.iter());
        let result = validate_args(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a directory"));
    }

    #[test]
    fn test_bedrock_path_must_exist() {
        let cmd = [
            "arnis",
            "--bedrock",
            "--output-dir",
            "/nonexistent/path",
            "--bbox",
            "1,2,3,4",
        ];
        let args = Args::parse_from(cmd.iter());
        let result = validate_args(&args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[test]
    fn test_required_options() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();

        let cmd = ["arnis"];
        assert!(Args::try_parse_from(cmd.iter()).is_err());

        let cmd = ["arnis", "--output-dir", tmp_path, "--bbox", "1,2,3,4"];
        let args = Args::try_parse_from(cmd.iter()).unwrap();
        assert!(validate_args(&args).is_ok());

        // Verify --path still works as a deprecated alias
        let cmd = ["arnis", "--path", tmp_path, "--bbox", "1,2,3,4"];
        let args = Args::try_parse_from(cmd.iter()).unwrap();
        assert!(validate_args(&args).is_ok());

        let cmd = ["arnis", "--output-dir", tmp_path, "--file", ""];
        assert!(Args::try_parse_from(cmd.iter()).is_err());

        // The --gui flag isn't used here, ugh. TODO clean up main.rs and its argparse usage.
        // let cmd = ["arnis", "--gui"];
        // assert!(Args::try_parse_from(cmd.iter()).is_ok());
    }

    #[test]
    fn test_spawn_point_both_required() {
        let tmpdir = tempfile::tempdir().unwrap();
        let tmp_path = tmpdir.path().to_str().unwrap();

        // Only spawn-lat without spawn-lng should fail validation
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--spawn-lat",
            "2.0",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(validate_args(&args).is_err());

        // Only spawn-lng without spawn-lat should fail validation
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--spawn-lng",
            "3.0",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(validate_args(&args).is_err());

        // Both provided and within bbox should pass
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--spawn-lat",
            "2.0",
            "--spawn-lng",
            "3.0",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(validate_args(&args).is_ok());

        // Spawn point outside bbox should fail
        let cmd = [
            "arnis",
            "--output-dir",
            tmp_path,
            "--bbox",
            "1,2,3,4",
            "--spawn-lat",
            "5.0",
            "--spawn-lng",
            "3.0",
        ];
        let args = Args::parse_from(cmd.iter());
        assert!(validate_args(&args).is_err());
    }
    /// The range is the fork's, not upstream's: Meld's scale box floors at 0.01 and its
    /// planet renders use it, so 0.01 must be accepted.
    #[test]
    fn scale_range_matches_the_forks_support_envelope() {
        for ok in [0.01, 0.05, 0.1, 0.5, 1.0, 2.0, 4.0] {
            assert!(validate_scale(ok).is_ok(), "{ok} should be accepted");
        }
        for bad in [0.0, -1.0, 0.009, 4.001, 99.0] {
            assert!(validate_scale(bad).is_err(), "{bad} should be rejected");
        }
    }

    /// These are the values that used to sail through and produce a hung or empty cell
    /// rather than an error message.
    #[test]
    fn scale_rejects_non_finite_values() {
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(validate_scale(bad).is_err(), "{bad} should be rejected");
        }
    }

    /// The parser must reject before clap hands us the value, so a bad scale never
    /// reaches the fetch stage.
    #[test]
    fn scale_is_rejected_at_parse_time() {
        assert!(parse_scale("0.1").is_ok());
        assert!(parse_scale("abc").is_err());
        assert!(parse_scale("0").is_err());
        assert!(parse_scale("-0.5").is_err());
        assert!(parse_scale("nan").is_err());
    }

    /// MELD-DIVERGENCE guard. Upstream skips every OSM/Overture object below scale 0.3.
    /// Meld's default is 0.1, so if that ever gets ported this test fails first.
    #[test]
    fn low_scale_does_not_skip_objects() {
        let tmp = tempfile::tempdir().unwrap();
        let tmp_path = tmp.path().to_str().unwrap();
        for scale in ["0.01", "0.1", "0.25", "0.29"] {
            let args = Args::parse_from([
                "arnis",
                "--output-dir",
                tmp_path,
                "--bbox",
                "1,2,3,4",
                "--scale",
                scale,
            ]);
            assert!(
                !args.skip_objects(),
                "scale {scale} must still render objects"
            );
        }
    }
}
