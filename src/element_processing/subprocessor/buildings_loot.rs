//! Data-driven chest loot tables.
//!
//! The built-in default (`LootConfig::default`) reproduces arnis's stock
//! weighted themes verbatim (the `THEMES` static below). A custom table can be
//! supplied at runtime with `--loot-table <file.json>`; `--dump-loot-table
//! <file.json>` writes the default out as JSON (the single source of truth for
//! Meld's web loot editor). Loot is deterministic per chest via `coord_rng`, so
//! it stays tile-invariant across Meld's per-cell renders.

use crate::deterministic_rng::coord_rng;
use fastnbt::Value;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// One weighted loot entry (owned / serde form used for the runtime config).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LootEntry {
    pub id: String,
    pub min: i32,
    pub max: i32,
    pub weight: u32,
}

/// A themed group of entries with its own selection weight.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LootTheme {
    pub weight: u32,
    pub items: Vec<LootEntry>,
}

/// Full runtime loot configuration (default = built-in, or loaded from JSON).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LootConfig {
    pub empty_weight: u32,
    pub rolls_min: u32,
    pub rolls_max: u32,
    pub themes: Vec<LootTheme>,
}

impl Default for LootConfig {
    fn default() -> Self {
        LootConfig {
            empty_weight: EMPTY_WEIGHT,
            rolls_min: 3,
            rolls_max: 8,
            themes: THEMES
                .iter()
                .map(|t| LootTheme {
                    weight: t.weight,
                    items: t
                        .items
                        .iter()
                        .map(|i| LootEntry {
                            id: i.id.to_string(),
                            min: i.min,
                            max: i.max,
                            weight: i.weight,
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

static LOOT: OnceLock<LootConfig> = OnceLock::new();

/// The active loot config (built-in default until `init_loot_from_path` sets one).
pub fn loot() -> &'static LootConfig {
    LOOT.get_or_init(LootConfig::default)
}

/// Validate a loaded config so a bad file can never crash a generation run.
fn validate(c: &LootConfig) -> Result<(), String> {
    if c.rolls_max < c.rolls_min {
        return Err("rolls_max must be >= rolls_min".into());
    }
    if c.themes.is_empty() {
        return Err("at least one theme is required".into());
    }
    for (ti, t) in c.themes.iter().enumerate() {
        if t.items.is_empty() {
            return Err(format!("theme {ti} has no items"));
        }
        let item_weight: u32 = t.items.iter().map(|i| i.weight).sum();
        if item_weight == 0 {
            return Err(format!("theme {ti} has zero total item weight"));
        }
        for (ii, it) in t.items.iter().enumerate() {
            if it.id.is_empty() || !it.id.contains(':') {
                return Err(format!("theme {ti} item {ii} has an invalid id '{}'", it.id));
            }
            if it.min < 0 || it.min > it.max || it.max > 64 {
                return Err(format!("theme {ti} item {ii} has an invalid count range"));
            }
        }
    }
    let theme_weight: u32 = c.themes.iter().map(|t| t.weight).sum();
    if theme_weight + c.empty_weight == 0 {
        return Err("total theme + empty weight must be > 0".into());
    }
    Ok(())
}

/// Load a custom loot table; on any error, warn and keep the built-in default.
/// Never panics or aborts a generation run.
pub fn init_loot_from_path(p: &Path) {
    let loaded = std::fs::read_to_string(p)
        .map_err(|e| e.to_string())
        .and_then(|s| serde_json::from_str::<LootConfig>(&s).map_err(|e| e.to_string()))
        .and_then(|c| validate(&c).map(|_| c));
    match loaded {
        Ok(c) => {
            let _ = LOOT.set(c);
        }
        Err(e) => {
            eprintln!(
                "Warning: --loot-table {} invalid ({e}); using built-in default",
                p.display()
            );
        }
    }
}

/// Write the built-in default table to `p` as pretty JSON (single source of
/// truth for the Meld loot editor's default + reset). Caller exits afterwards.
pub fn dump_default_loot_table(p: &Path) {
    let s =
        serde_json::to_string_pretty(&LootConfig::default()).expect("serialize default loot table");
    if let Err(e) = std::fs::write(p, s) {
        eprintln!("Error: could not write loot table to {}: {e}", p.display());
        std::process::exit(1);
    }
}

fn pick_entry<'a>(theme: &'a LootTheme, rng: &mut impl Rng) -> &'a LootEntry {
    let total: u32 = theme.items.iter().map(|i| i.weight).sum();
    let mut pick = rng.random_range(0..total);
    for item in &theme.items {
        if pick < item.weight {
            return item;
        }
        pick -= item.weight;
    }
    &theme.items[theme.items.len() - 1]
}

/// Deterministic per-chest loot keyed on world coords; a few scattered stacks per chest.
pub fn chest_loot(x: i32, z: i32, salt: u32) -> Vec<HashMap<String, Value>> {
    let cfg = loot();
    let mut rng = coord_rng(x, z, salt as u64 ^ 0x1007_C0DE);
    let rolls = rng.random_range(cfg.rolls_min..=cfg.rolls_max);
    let theme_total: u32 = cfg.themes.iter().map(|t| t.weight).sum::<u32>() + cfg.empty_weight;
    if theme_total == 0 {
        return Vec::new();
    }

    let mut used = [false; CHEST_SLOTS];
    let mut out = Vec::new();

    for _ in 0..rolls {
        let mut pick = rng.random_range(0..theme_total);
        if pick < cfg.empty_weight {
            continue;
        }
        pick -= cfg.empty_weight;

        let mut chosen = &cfg.themes[0];
        for theme in &cfg.themes {
            if pick < theme.weight {
                chosen = theme;
                break;
            }
            pick -= theme.weight;
        }

        let item = pick_entry(chosen, &mut rng);
        let count = rng.random_range(item.min..=item.max);

        let mut slot = None;
        for _ in 0..4 {
            let candidate = rng.random_range(0..CHEST_SLOTS);
            if !used[candidate] {
                slot = Some(candidate);
                break;
            }
        }
        let Some(slot) = slot else { continue };
        used[slot] = true;

        // 1.20.5+ container item format: lowercase count (Int).
        let mut item_nbt = HashMap::new();
        item_nbt.insert("id".to_string(), Value::String(item.id.clone()));
        item_nbt.insert("Slot".to_string(), Value::Byte(slot as i8));
        item_nbt.insert("count".to_string(), Value::Int(count));
        out.push(item_nbt);
    }

    out
}

// ---------------------------------------------------------------------------
// Built-in default loot tables (verbatim from upstream arnis). Serves only as
// the source for `LootConfig::default`; runtime selection reads `loot()`.
// ---------------------------------------------------------------------------
// Rarity weights applied per item within its theme.
const COMMON: u32 = 9;
const UNCOMMON: u32 = 3;
const RARE: u32 = 1;

// Some rolls place nothing so chests are not always full.
const EMPTY_WEIGHT: u32 = 3;

const CHEST_SLOTS: usize = 27;

struct LootItem {
    id: &'static str,
    min: i32,
    max: i32,
    weight: u32,
}

struct Theme {
    weight: u32,
    items: &'static [LootItem],
}

// Stackable items use bigger counts; tools, armour and treasure stay single.
const THEMES: &[Theme] = &[
    // Food and kitchen.
    Theme {
        weight: 25,
        items: &[
            LootItem {
                id: "minecraft:bread",
                min: 2,
                max: 6,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:potato",
                min: 3,
                max: 9,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:carrot",
                min: 2,
                max: 7,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:wheat",
                min: 3,
                max: 9,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:apple",
                min: 2,
                max: 6,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:baked_potato",
                min: 2,
                max: 6,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:cooked_chicken",
                min: 1,
                max: 4,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:sweet_berries",
                min: 2,
                max: 7,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:beetroot",
                min: 2,
                max: 6,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:pumpkin_pie",
                min: 1,
                max: 3,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:mushroom_stew",
                min: 1,
                max: 1,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:golden_carrot",
                min: 1,
                max: 3,
                weight: RARE,
            },
            LootItem {
                id: "minecraft:cake",
                min: 1,
                max: 1,
                weight: RARE,
            },
        ],
    },
    // Junk and flavour.
    Theme {
        weight: 20,
        items: &[
            LootItem {
                id: "minecraft:paper",
                min: 2,
                max: 7,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:bone",
                min: 2,
                max: 7,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:string",
                min: 2,
                max: 7,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:rotten_flesh",
                min: 2,
                max: 6,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:book",
                min: 1,
                max: 4,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:dead_bush",
                min: 1,
                max: 3,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:gunpowder",
                min: 1,
                max: 4,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:flower_pot",
                min: 1,
                max: 1,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:cobweb",
                min: 1,
                max: 3,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:name_tag",
                min: 1,
                max: 1,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:map",
                min: 1,
                max: 1,
                weight: UNCOMMON,
            },
        ],
    },
    // Building resources.
    Theme {
        weight: 18,
        items: &[
            LootItem {
                id: "minecraft:oak_planks",
                min: 4,
                max: 16,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:cobblestone",
                min: 6,
                max: 20,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:coal",
                min: 3,
                max: 9,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:clay_ball",
                min: 2,
                max: 7,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:glass_pane",
                min: 3,
                max: 9,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:torch",
                min: 2,
                max: 8,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:iron_ingot",
                min: 2,
                max: 6,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:candle",
                min: 1,
                max: 4,
                weight: UNCOMMON,
            },
        ],
    },
    // Tools and utility.
    Theme {
        weight: 15,
        items: &[
            LootItem {
                id: "minecraft:stick",
                min: 2,
                max: 7,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:bucket",
                min: 1,
                max: 1,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:fishing_rod",
                min: 1,
                max: 1,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:shears",
                min: 1,
                max: 1,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:flint_and_steel",
                min: 1,
                max: 1,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:compass",
                min: 1,
                max: 1,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:iron_pickaxe",
                min: 1,
                max: 1,
                weight: RARE,
            },
            LootItem {
                id: "minecraft:iron_axe",
                min: 1,
                max: 1,
                weight: RARE,
            },
            LootItem {
                id: "minecraft:clock",
                min: 1,
                max: 1,
                weight: RARE,
            },
        ],
    },
    // Valuables and treasure.
    Theme {
        weight: 12,
        items: &[
            LootItem {
                id: "minecraft:iron_nugget",
                min: 3,
                max: 8,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:gold_nugget",
                min: 2,
                max: 7,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:lapis_lazuli",
                min: 2,
                max: 6,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:emerald",
                min: 1,
                max: 4,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:gold_ingot",
                min: 1,
                max: 3,
                weight: RARE,
            },
            LootItem {
                id: "minecraft:amethyst_shard",
                min: 1,
                max: 4,
                weight: RARE,
            },
            LootItem {
                id: "minecraft:diamond",
                min: 1,
                max: 2,
                weight: RARE,
            },
        ],
    },
    // Adventure gear.
    Theme {
        weight: 10,
        items: &[
            LootItem {
                id: "minecraft:arrow",
                min: 3,
                max: 12,
                weight: COMMON,
            },
            LootItem {
                id: "minecraft:leather_boots",
                min: 1,
                max: 1,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:leather_chestplate",
                min: 1,
                max: 1,
                weight: UNCOMMON,
            },
            LootItem {
                id: "minecraft:shield",
                min: 1,
                max: 1,
                weight: RARE,
            },
            LootItem {
                id: "minecraft:golden_apple",
                min: 1,
                max: 1,
                weight: RARE,
            },
            LootItem {
                id: "minecraft:ender_pearl",
                min: 1,
                max: 3,
                weight: RARE,
            },
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_round_trips_through_serde() {
        let d = LootConfig::default();
        let s = serde_json::to_string(&d).unwrap();
        let back: LootConfig = serde_json::from_str(&s).unwrap();
        validate(&back).unwrap();
        assert_eq!(back.themes.len(), d.themes.len());
        assert!(!d.themes.is_empty());
    }

    #[test]
    fn chest_loot_is_deterministic() {
        let a = chest_loot(1234, -987, 7);
        let b = chest_loot(1234, -987, 7);
        assert_eq!(a.len(), b.len());
        for (ia, ib) in a.iter().zip(b.iter()) {
            assert_eq!(ia.get("id"), ib.get("id"));
            assert_eq!(ia.get("Slot"), ib.get("Slot"));
            assert_eq!(ia.get("count"), ib.get("count"));
        }
    }
}
