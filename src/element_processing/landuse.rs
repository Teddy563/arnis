use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::deterministic_rng::element_rng;
use crate::element_processing::bridges::BridgeSurfaceMap;
use crate::element_processing::field_texture::{FieldCategory, FieldMix};
use crate::element_processing::tree::{Tree, TreeType};
use crate::floodfill_cache::{BuildingFootprintBitmap, FloodFillCache, RoadMaskBitmap};
use crate::osm_parser::{ProcessedMemberRole, ProcessedRelation, ProcessedWay};
use crate::world_editor::WorldEditor;
use rand::prelude::IndexedRandom;
use rand::Rng;

pub fn generate_landuse(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
    road_mask: &RoadMaskBitmap,
    bridge_surface: &BridgeSurfaceMap,
) {
    // Determine block type based on landuse tag
    let binding: String = "".to_string();
    let landuse_tag: &String = element.tags.get("landuse").unwrap_or(&binding);

    // Single-world: one id-seeded stream. Tile mode (Meld): reseeded per tile in
    // the fill loop below so terrain-gated scatter can't desync a shared stream
    // across cells and cascade at the seam.
    let mut stream_rng = element_rng(element.id);
    let tile_inv = crate::ground_generation::tile_invariant_enabled();

    // Farmland texture mix. Default (flag omitted) => all-farm => stock behaviour.
    let field_mix = FieldMix::parse(args.field_mix.as_deref());
    let field_mix_active = landuse_tag == "farmland" && !field_mix.is_default();

    let block_type = match landuse_tag.as_str() {
        "greenfield" | "meadow" | "grass" | "orchard" | "forest" => GRASS_BLOCK,
        "farmland" => FARMLAND,
        "cemetery" => PODZOL,
        "construction" => COARSE_DIRT,
        "traffic_island" => STONE_BLOCK_SLAB,
        // residential and commercial are too broad, they cover entire zones including
        // gardens, parks, and green spaces. ESA WorldCover handles built-up classification
        // at 10m satellite resolution, which is far more precise.
        "residential" | "commercial" => return,
        "education" => POLISHED_ANDESITE,
        "religious" => POLISHED_ANDESITE,
        "industrial" => STONE,       // Randomized per-block below
        "military" => GRAY_CONCRETE, // Randomized per-block below
        "railway" => GRAVEL,
        "vineyard" => COARSE_DIRT,
        "brownfield" => COARSE_DIRT,
        "farmyard" => COARSE_DIRT,
        "landfill" => {
            // Gravel if man_made = spoil_heap or heap, coarse dirt else
            let manmade_tag = element.tags.get("man_made").unwrap_or(&binding);
            if manmade_tag == "spoil_heap" || manmade_tag == "heap" {
                GRAVEL
            } else {
                COARSE_DIRT
            }
        }
        "quarry" => STONE, // Randomized per-block below
        _ => GRASS_BLOCK,
    };

    // Get the area of the landuse element using cache
    let floor_area = flood_fill_cache.get_or_compute(element, args.timeout.as_ref());

    // Cherry/FloweringOak only via the random Tree::create pool (rare).
    let trees_ok_to_generate: Vec<TreeType> = {
        let mut trees: Vec<TreeType> = vec![];
        if let Some(leaf_type) = element.tags.get("leaf_type") {
            match leaf_type.as_str() {
                "broadleaved" => {
                    trees.push(TreeType::Oak);
                    trees.push(TreeType::Birch);
                    trees.push(TreeType::TallOak);
                    trees.push(TreeType::Bush);
                    trees.push(TreeType::AzaleaBush);
                }
                "needleleaved" => {
                    trees.push(TreeType::Spruce);
                    trees.push(TreeType::Pine);
                }
                _ => {
                    trees.push(TreeType::Oak);
                    trees.push(TreeType::Spruce);
                    trees.push(TreeType::Birch);
                    trees.push(TreeType::TallOak);
                    trees.push(TreeType::Pine);
                    trees.push(TreeType::Bush);
                    trees.push(TreeType::AzaleaBush);
                    trees.push(TreeType::Willow);
                }
            }
        } else {
            trees.push(TreeType::Oak);
            trees.push(TreeType::Spruce);
            trees.push(TreeType::Birch);
            trees.push(TreeType::TallOak);
            trees.push(TreeType::Pine);
            trees.push(TreeType::Bush);
            trees.push(TreeType::AzaleaBush);
        }
        trees
    };

    for &(x, z) in floor_area.iter() {
        // Per-tile RNG: in tile mode use a position-only coord_rng so the scatter
        // arms below (which draw the RNG inside terrain gates like
        // check_for_block(GRASS_BLOCK)) can't desync a shared id stream across
        // cells and cascade a different scatter across the whole area at every
        // seam. Single-world keeps the id-seeded stream → byte-identical.
        let mut tile_rng;
        let rng = if tile_inv {
            tile_rng = crate::deterministic_rng::coord_rng(x, z, element.id);
            &mut tile_rng
        } else {
            &mut stream_rng
        };
        // Farmland patch category for this cell (Farm elsewhere / when inactive).
        let field_cat = if field_mix_active {
            field_mix.category_at(x, z)
        } else {
            FieldCategory::Farm
        };
        // Apply per-block randomness for certain landuse types
        let actual_block = if landuse_tag == "industrial" {
            // Industrial: primarily stone, with some stone bricks and smooth stone
            let random_value = rng.random_range(0..100);
            if random_value < 70 {
                STONE
            } else if random_value < 90 {
                STONE_BRICKS
            } else {
                SMOOTH_STONE
            }
        } else if landuse_tag == "military" {
            // Military: primarily gray concrete, with some stone bricks and cobblestone
            let random_value = rng.random_range(0..100);
            if random_value < 89 {
                GRAY_CONCRETE
            } else if random_value < 99 {
                STONE_BRICKS
            } else {
                COBBLESTONE
            }
        } else if landuse_tag == "quarry" {
            // Quarry: mix of stone, gravel, cobblestone, andesite
            let random_value = rng.random_range(0..100);
            if random_value < 40 {
                STONE
            } else if random_value < 60 {
                GRAVEL
            } else if random_value < 80 {
                COBBLESTONE
            } else {
                ANDESITE
            }
        } else {
            block_type
        };
        // Override the farmland surface per patch category when the mix is active.
        let actual_block = if field_mix_active {
            match field_cat {
                FieldCategory::Coarse => COARSE_DIRT,
                FieldCategory::Moss => MOSS_BLOCK,
                FieldCategory::Plains | FieldCategory::Flower => GRASS_BLOCK,
                FieldCategory::Farm => FARMLAND,
            }
        } else {
            actual_block
        };

        // Don't overwrite roads or water with landuse ground blocks
        let is_protected = editor.check_for_block(
            x,
            0,
            z,
            Some(&[
                BLACK_CONCRETE,
                GRAY_CONCRETE_POWDER,
                CYAN_TERRACOTTA,
                GRAY_CONCRETE,
                LIGHT_GRAY_CONCRETE,
                WHITE_CONCRETE,
                DIRT_PATH,
                SMOOTH_STONE,
                WATER,
            ]),
        );

        if landuse_tag == "traffic_island" {
            editor.set_block(actual_block, x, 1, z, None, None);
        } else if landuse_tag == "construction" || landuse_tag == "railway" {
            editor.set_block(actual_block, x, 0, z, None, Some(&[SPONGE]));
        } else if !is_protected {
            editor.set_block(actual_block, x, 0, z, None, None);
        }

        // Add specific features for different landuse types
        match landuse_tag.as_str() {
            "cemetery" if (x % 3 == 0) && (z % 3 == 0) => {
                let random_choice: i32 = rng.random_range(0..100);
                if random_choice < 15 {
                    // Bundled .schem tombstone (replaces the old procedural grave;
                    // self-limits and avoids roads via the road mask internally).
                    crate::structures::tombstone::maybe_place(editor, x, z, road_mask);
                } else if random_choice < 30 {
                    if editor.check_for_block(x, 0, z, Some(&[PODZOL])) {
                        editor.set_block(RED_FLOWER, x, 1, z, None, None);
                    }
                } else if random_choice < 33 {
                    Tree::create(
                        editor,
                        (x, 1, z),
                        Some(building_footprints),
                        Some(bridge_surface),
                    );
                } else if !is_protected && random_choice < 35 {
                    editor.set_block(OAK_LEAVES, x, 1, z, None, None);
                } else if !is_protected && random_choice < 37 {
                    editor.set_block(FERN, x, 1, z, None, None);
                } else if !is_protected && random_choice < 41 {
                    editor.set_block(LARGE_FERN_LOWER, x, 1, z, None, None);
                    editor.set_block(LARGE_FERN_UPPER, x, 2, z, None, None);
                }
            }
            "forest" if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) => {
                // Density-modulated spawn: thickets in some patches, clearings in
                // others. Uses the loop's per-tile `rng` (position-only in tile
                // mode), so the same world tile resolves identically in any cell.
                let density = crate::ground_generation::value_noise_01(x, z, 32);
                let tree_threshold = ((60.0 - density * 45.0) as i32).max(5);
                if rng.random_range(0..tree_threshold) == 0 {
                    let tree_type = *trees_ok_to_generate
                        .choose(&mut *rng)
                        .unwrap_or(&TreeType::Oak);
                    Tree::create_of_type(
                        editor,
                        (x, 1, z),
                        tree_type,
                        Some(building_footprints),
                        Some(bridge_surface),
                        false,
                    );
                } else {
                    let random_choice: i32 = rng.random_range(0..30);
                    if random_choice == 2 {
                        let flower_block: Block = match rng.random_range(1..=6) {
                            1 => OAK_LEAVES,
                            2 => RED_FLOWER,
                            3 => BLUE_FLOWER,
                            4 => YELLOW_FLOWER,
                            5 => FERN,
                            _ => WHITE_FLOWER,
                        };
                        editor.set_block(flower_block, x, 1, z, None, None);
                    } else if random_choice <= 12 {
                        if rng.random_range(0..100) < 12 {
                            editor.set_block(FERN, x, 1, z, None, None);
                        } else {
                            editor.set_block(GRASS, x, 1, z, None, None);
                        }
                    }
                }
            }
            "farmland" if !editor.check_for_block(x, 0, z, Some(&[WATER])) => {
                // Decoration follows the patch category. Farm = stock behaviour
                // (byte-identical when --field-mix is omitted, since field_cat is
                // then always Farm); the others texture the alternate surfaces.
                match field_cat {
                    FieldCategory::Farm => {
                        // Check if the current block is not water or another undesired block
                        if x % 9 == 0 && z % 9 == 0 && editor.water_source_is_enclosed(x, z) {
                            // Place water in dot pattern only where it sits in a basin, so on sloped
                            // fields it can't run downhill and wash out the crops (upstream 046a746).
                            editor.set_block(WATER, x, 0, z, Some(&[FARMLAND]), None);
                        } else if rng.random_range(0..76) == 0 {
                            let special_choice: i32 = rng.random_range(1..=10);
                            if special_choice <= 4 {
                                editor.set_block(HAY_BALE, x, 1, z, None, Some(&[SPONGE]));
                            } else {
                                editor.set_block(OAK_LEAVES, x, 1, z, None, Some(&[SPONGE]));
                            }
                        } else {
                            // Set crops only if the block below is farmland
                            if editor.check_for_block(x, 0, z, Some(&[FARMLAND])) {
                                let crop_choice = [WHEAT, CARROTS, POTATOES][rng.random_range(0..3)];
                                editor.set_block(crop_choice, x, 1, z, None, None);
                            }
                        }
                    }
                    FieldCategory::Plains => match rng.random_range(0..1000) {
                        0..=6 => {
                            editor.set_block(TALL_GRASS_BOTTOM, x, 1, z, None, None);
                            editor.set_block(TALL_GRASS_TOP, x, 2, z, None, None);
                        }
                        7..=560 => editor.set_block(GRASS, x, 1, z, None, None),
                        _ => {}
                    },
                    FieldCategory::Flower => match rng.random_range(0..1000) {
                        0..=45 => {
                            let flower = [RED_FLOWER, YELLOW_FLOWER, BLUE_FLOWER, WHITE_FLOWER]
                                [rng.random_range(0..4)];
                            editor.set_block(flower, x, 1, z, None, None);
                        }
                        46..=520 => editor.set_block(GRASS, x, 1, z, None, None),
                        _ => {}
                    },
                    FieldCategory::Coarse => match rng.random_range(0..150) {
                        0..=3 => editor.set_block(DEAD_BUSH, x, 1, z, None, None),
                        4..=12 => editor.set_block(GRASS, x, 1, z, None, None),
                        _ => {}
                    },
                    FieldCategory::Moss => match rng.random_range(0..100) {
                        0..=2 => editor.set_block(AZALEA, x, 1, z, None, None),
                        3..=22 => editor.set_block(MOSS_CARPET, x, 1, z, None, None),
                        23..=45 => editor.set_block(GRASS, x, 1, z, None, None),
                        _ => {}
                    },
                }
            }
            "construction" => {
                let random_choice: i32 = rng.random_range(0..1501);
                if random_choice < 15 {
                    editor.set_block(SCAFFOLDING, x, 1, z, None, None);
                    if random_choice < 2 {
                        editor.set_block(SCAFFOLDING, x, 2, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 3, z, None, None);
                    } else if random_choice < 4 {
                        editor.set_block(SCAFFOLDING, x, 2, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 3, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 4, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 1, z + 1, None, None);
                    } else {
                        editor.set_block(SCAFFOLDING, x, 2, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 3, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 4, z, None, None);
                        editor.set_block(SCAFFOLDING, x, 5, z, None, None);
                        editor.set_block(SCAFFOLDING, x - 1, 1, z, None, None);
                        editor.set_block(SCAFFOLDING, x + 1, 1, z - 1, None, None);
                    }
                } else if random_choice < 55 {
                    let construction_items: [Block; 13] = [
                        OAK_LOG,
                        COBBLESTONE,
                        GRAVEL,
                        GLOWSTONE,
                        STONE,
                        COBBLESTONE_WALL,
                        BLACK_CONCRETE,
                        SAND,
                        OAK_PLANKS,
                        DIRT,
                        BRICK,
                        CRAFTING_TABLE,
                        FURNACE,
                    ];
                    editor.set_block(
                        construction_items[rng.random_range(0..construction_items.len())],
                        x,
                        1,
                        z,
                        None,
                        None,
                    );
                } else if random_choice < 65 {
                    if random_choice < 60 {
                        editor.set_block(DIRT, x, 1, z, None, None);
                        editor.set_block(DIRT, x, 2, z, None, None);
                        editor.set_block(DIRT, x + 1, 1, z, None, None);
                        editor.set_block(DIRT, x, 1, z + 1, None, None);
                    } else {
                        editor.set_block(DIRT, x, 1, z, None, None);
                        editor.set_block(DIRT, x, 2, z, None, None);
                        editor.set_block(DIRT, x - 1, 1, z, None, None);
                        editor.set_block(DIRT, x, 1, z - 1, None, None);
                    }
                } else if random_choice < 100 {
                    editor.set_block(GRAVEL, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 115 {
                    editor.set_block(SAND, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 125 {
                    editor.set_block(DIORITE, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 145 {
                    editor.set_block(BRICK, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 155 {
                    editor.set_block(GRANITE, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 180 {
                    editor.set_block(ANDESITE, x, 0, z, None, Some(&[SPONGE]));
                } else if random_choice < 565 {
                    editor.set_block(COBBLESTONE, x, 0, z, None, Some(&[SPONGE]));
                }
            }
            "grass" if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) => {
                match rng.random_range(0..200) {
                    0 => editor.set_block(OAK_LEAVES, x, 1, z, None, None),
                    1..=8 => editor.set_block(FERN, x, 1, z, None, None),
                    9..=170 => editor.set_block(GRASS, x, 1, z, None, None),
                    _ => {}
                }
            }
            "greenfield" if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) => {
                match rng.random_range(0..200) {
                    0 => editor.set_block(OAK_LEAVES, x, 1, z, None, None),
                    1..=2 => editor.set_block(FERN, x, 1, z, None, None),
                    3..=16 => editor.set_block(GRASS, x, 1, z, None, None),
                    _ => {}
                }
            }
            "meadow" if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) => {
                let random_choice: i32 = rng.random_range(0..1001);
                if random_choice < 5 {
                    Tree::create(
                        editor,
                        (x, 1, z),
                        Some(building_footprints),
                        Some(bridge_surface),
                    );
                } else if random_choice < 6 {
                    editor.set_block(RED_FLOWER, x, 1, z, None, None);
                } else if random_choice < 9 {
                    editor.set_block(OAK_LEAVES, x, 1, z, None, None);
                } else if random_choice < 40 {
                    editor.set_block(FERN, x, 1, z, None, None);
                } else if random_choice < 65 {
                    editor.set_block(LARGE_FERN_LOWER, x, 1, z, None, None);
                    editor.set_block(LARGE_FERN_UPPER, x, 2, z, None, None);
                } else if random_choice < 825 {
                    editor.set_block(GRASS, x, 1, z, None, None);
                }
            }
            "orchard" => {
                if x % 18 == 0 && z % 10 == 0 {
                    Tree::create(
                        editor,
                        (x, 1, z),
                        Some(building_footprints),
                        Some(bridge_surface),
                    );
                } else if editor.check_for_block(x, 0, z, Some(&[GRASS_BLOCK])) {
                    match rng.random_range(0..100) {
                        0 => editor.set_block(OAK_LEAVES, x, 1, z, None, None),
                        1..=2 => editor.set_block(FERN, x, 1, z, None, None),
                        3..=20 => editor.set_block(GRASS, x, 1, z, None, None),
                        _ => {}
                    }
                }
            }
            "vineyard" | "brownfield" | "landfill"
                if editor.check_for_block(x, 0, z, Some(&[COARSE_DIRT])) =>
            {
                // Sparse weeds/regrowth on coarse-dirt surfaces: vineyard rows
                // grow some grass between vines, and brownfield/landfill are
                // abandoned land that nature is slowly reclaiming. Kept rare so
                // the ground still reads as dry/disturbed rather than meadow.
                // (Skipped for landfill spoil heaps — those are GRAVEL, not
                // COARSE_DIRT, and the guard above filters them out.)
                match rng.random_range(0..150) {
                    0..=3 => editor.set_block(OAK_LEAVES, x, 1, z, None, None),
                    4 => editor.set_block(DEAD_BUSH, x, 1, z, None, None),
                    5..=15 => editor.set_block(GRASS, x, 1, z, None, None),
                    _ => {}
                }
            }
            "quarry" => {
                // Add stone layer under it
                editor.set_block(STONE, x, -1, z, Some(&[STONE]), None);
                editor.set_block(STONE, x, -2, z, Some(&[STONE]), None);
                // Generate ore blocks
                if let Some(resource) = element.tags.get("resource") {
                    let ore_block = match resource.as_str() {
                        "iron_ore" => IRON_ORE,
                        "coal" => COAL_ORE,
                        "copper" => COPPER_ORE,
                        "gold" => GOLD_ORE,
                        "clay" | "kaolinite" => CLAY,
                        _ => STONE,
                    };
                    let random_choice: i32 =
                        rng.random_range(0..100 + editor.get_absolute_y(x, 0, z)); // The deeper it is the more resources are there
                    if random_choice < 5 {
                        editor.set_block(ore_block, x, 0, z, Some(&[STONE]), None);
                    }
                }
            }
            _ => {}
        }
    }

    // Generate a stone brick wall fence around cemeteries
    if landuse_tag == "cemetery" {
        generate_cemetery_fence(editor, element);
    }

    // Large construction sites get a centre crane plus scattered excavators;
    // farmland gets an occasional parked tractor.
    if landuse_tag == "construction" {
        crate::structures::crane::maybe_place_crane(editor, floor_area.as_slice());
        crate::structures::excavator::scatter_excavators(editor, floor_area.as_slice());
    }
    if landuse_tag == "farmland" {
        crate::structures::tractor::maybe_place_tractor(editor, floor_area.as_slice());
        // Optional scattered rocks/bushes (off by default; density 0 = no-op).
        let rock_density = if args.rocks { args.rock_density } else { 0 };
        let bush_density = if args.bushes { args.bush_density } else { 0 };
        crate::structures::rocks::scatter_rocks(editor, floor_area.as_slice(), rock_density);
        crate::structures::bushes::scatter_bushes(editor, floor_area.as_slice(), bush_density);
    }
}

/// Draws a stone-brick wall fence (with slab cap) along the outline of a
/// cemetery way.
fn generate_cemetery_fence(editor: &mut WorldEditor, element: &ProcessedWay) {
    for i in 1..element.nodes.len() {
        let prev = &element.nodes[i - 1];
        let cur = &element.nodes[i];

        let points = bresenham_line(prev.x, 0, prev.z, cur.x, 0, cur.z);
        for (bx, _, bz) in points {
            editor.set_block(STONE_BRICK_WALL, bx, 1, bz, None, None);
            editor.set_block(STONE_BRICK_SLAB, bx, 2, bz, None, None);
        }
    }
}

pub fn generate_landuse_from_relation(
    editor: &mut WorldEditor,
    rel: &ProcessedRelation,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
    building_footprints: &BuildingFootprintBitmap,
    road_mask: &RoadMaskBitmap,
    bridge_surface: &BridgeSurfaceMap,
) {
    if rel.tags.contains_key("landuse") {
        // Process each outer member way individually using cached flood fill.
        // We intentionally do not combine all outer nodes into one mega-way,
        // because that creates a nonsensical polygon spanning the whole relation
        // extent, misses the flood fill cache, and can cause multi-GB allocations.
        for member in &rel.members {
            if member.role == ProcessedMemberRole::Outer {
                // Use relation tags so the member inherits the relation's landuse=* type
                let way_with_rel_tags = ProcessedWay {
                    id: member.way.id,
                    nodes: member.way.nodes.clone(),
                    tags: rel.tags.clone(),
                    unclipped_bounds: member.way.unclipped_bounds,
                    unclipped_polygon_area: member.way.unclipped_polygon_area,
                };
                generate_landuse(
                    editor,
                    &way_with_rel_tags,
                    args,
                    flood_fill_cache,
                    building_footprints,
                    road_mask,
                    bridge_surface,
                );
            }
        }
    }
}

/// Generates ground blocks for place=* areas (squares, neighbourhoods, etc.)
pub fn generate_place(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    args: &Args,
    flood_fill_cache: &FloodFillCache,
) {
    let binding = String::new();
    let place_tag = element.tags.get("place").unwrap_or(&binding);

    // Determine block type based on place tag
    let block_type = match place_tag.as_str() {
        "square" => STONE_BRICKS,
        // neighbourhood/city_block/quarter/suburb are too broad, ESA WorldCover
        // land cover data handles built-up classification at 10m resolution instead
        "neighbourhood" | "city_block" | "quarter" | "suburb" => return,
        _ => return,
    };

    // Get the area using flood fill cache
    let floor_area = flood_fill_cache.get_or_compute(element, args.timeout.as_ref());

    // Place ground blocks
    for &(x, z) in floor_area.iter() {
        editor.set_block(block_type, x, 0, z, None, None);
    }
}
