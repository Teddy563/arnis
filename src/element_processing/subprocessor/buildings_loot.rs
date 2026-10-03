//! Chest loot for building interiors. The built-in table below can be replaced at run
//! time with `--loot-table <file.json>`; `--dump-loot-table <file.json>` writes the
//! built-in one out in that format as a starting point.

use crate::deterministic_rng::coord_rng;
use fastnbt::Value;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock, PoisonError, RwLock};

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

/// One weighted item of a theme, as read from a `--loot-table` file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LootEntry {
    pub id: String,
    pub min: i32,
    pub max: i32,
    pub weight: u32,
}

/// A group of items picked together, weighted against the other themes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LootTheme {
    pub weight: u32,
    pub items: Vec<LootEntry>,
}

/// A whole chest loot table: how many stacks to roll and what they can be.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LootTable {
    pub empty_weight: u32,
    pub rolls_min: u32,
    pub rolls_max: u32,
    pub themes: Vec<LootTheme>,
}

impl Default for LootTable {
    fn default() -> Self {
        LootTable {
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

impl LootTable {
    /// Rejects anything `chest_loot` could not roll from without panicking.
    fn validate(&self) -> Result<(), String> {
        if self.rolls_min > self.rolls_max || self.rolls_max > 64 {
            return Err("rolls must satisfy rolls_min <= rolls_max <= 64".to_string());
        }
        if self.themes.is_empty() {
            return Err("at least one theme is required".to_string());
        }
        for (ti, theme) in self.themes.iter().enumerate() {
            if theme.items.iter().map(|i| i.weight as u64).sum::<u64>() == 0 {
                return Err(format!("theme {ti} has no item with a weight above 0"));
            }
            for (ii, item) in theme.items.iter().enumerate() {
                if !item.id.contains(':') {
                    return Err(format!(
                        "theme {ti} item {ii}: '{}' is not a namespaced id like minecraft:bread",
                        item.id
                    ));
                }
                if item.min < 0 || item.min > item.max || item.max > 64 {
                    return Err(format!(
                        "theme {ti} item {ii}: count must satisfy 0 <= min <= max <= 64"
                    ));
                }
            }
        }
        let total: u64 =
            self.themes.iter().map(|t| t.weight as u64).sum::<u64>() + self.empty_weight as u64;
        if total == 0 || total > u32::MAX as u64 {
            return Err("theme weights plus empty_weight must be between 1 and 2^32-1".to_string());
        }
        Ok(())
    }
}

static BUILT_IN: LazyLock<Arc<LootTable>> = LazyLock::new(|| Arc::new(LootTable::default()));

// Replaceable rather than set-once: the GUI runs several generations in one process.
static ACTIVE: RwLock<Option<Arc<LootTable>>> = RwLock::new(None);

/// Reads and checks a `--loot-table` file.
pub fn load_loot_table(path: &Path) -> Result<LootTable, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let table: LootTable = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    table.validate()?;
    Ok(table)
}

/// Sets the table chests roll from for the next generation; `None` restores the built-in one.
pub fn set_loot_table(table: Option<LootTable>) {
    *ACTIVE.write().unwrap_or_else(PoisonError::into_inner) = table.map(Arc::new);
}

/// The built-in table as pretty JSON, in the format `--loot-table` reads.
pub fn built_in_loot_table_json() -> String {
    serde_json::to_string_pretty(&**BUILT_IN).expect("built-in loot table serializes")
}

fn active_table() -> Arc<LootTable> {
    ACTIVE
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .unwrap_or_else(|| BUILT_IN.clone())
}

fn pick_item<'a>(theme: &'a LootTheme, rng: &mut impl Rng) -> &'a LootEntry {
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
    roll_chest(&active_table(), x, z, salt)
}

fn roll_chest(table: &LootTable, x: i32, z: i32, salt: u32) -> Vec<HashMap<String, Value>> {
    let mut rng = coord_rng(x, z, salt as u64 ^ 0x1007_C0DE);
    let rolls = rng.random_range(table.rolls_min..=table.rolls_max);
    let theme_total: u32 = table.themes.iter().map(|t| t.weight).sum::<u32>() + table.empty_weight;

    let mut used = [false; CHEST_SLOTS];
    let mut out = Vec::new();

    for _ in 0..rolls {
        let mut pick = rng.random_range(0..theme_total);
        if pick < table.empty_weight {
            continue;
        }
        pick -= table.empty_weight;

        let mut chosen = &table.themes[0];
        for theme in &table.themes {
            if pick < theme.weight {
                chosen = theme;
                break;
            }
            pick -= theme.weight;
        }
        let item = pick_item(chosen, &mut rng);
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

        // 1.20.5+ container item format: lowercase count (Int), matching the map chest.
        let mut item_nbt = HashMap::new();
        item_nbt.insert("id".to_string(), Value::String(item.id.clone()));
        item_nbt.insert("Slot".to_string(), Value::Byte(slot as i8));
        item_nbt.insert("count".to_string(), Value::Int(count));
        out.push(item_nbt);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // FNV-1a over every (slot, id, count) a grid of chests rolls.
    fn fingerprint(table: &LootTable) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for x in (-300..300).step_by(7) {
            for z in (-300..300).step_by(11) {
                for salt in [0, 1, 0xBEEF] {
                    for item in roll_chest(table, x, z, salt) {
                        let line = format!("{:?}{:?}{:?}", item["Slot"], item["id"], item["count"]);
                        for b in line.bytes() {
                            h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
                        }
                    }
                }
            }
        }
        h
    }

    #[test]
    fn built_in_table_rolls_what_the_hardcoded_themes_rolled() {
        // Captured from the const THEMES code before the table became data.
        assert_eq!(fingerprint(&LootTable::default()), 0x44e0_ead9_45e0_1a04);
    }

    #[test]
    fn dump_round_trips_and_validates() {
        let back: LootTable = serde_json::from_str(&built_in_loot_table_json()).unwrap();
        assert_eq!(back, LootTable::default());
        back.validate().unwrap();
    }

    #[test]
    fn validate_rejects_tables_that_could_panic() {
        let base = LootTable::default();
        let mut t = base.clone();
        t.rolls_min = 9;
        t.rolls_max = 2;
        assert!(t.validate().is_err());
        let mut t = base.clone();
        t.themes.clear();
        assert!(t.validate().is_err());
        let mut t = base.clone();
        t.themes[0].items.iter_mut().for_each(|i| i.weight = 0);
        assert!(t.validate().is_err());
        let mut t = base.clone();
        t.themes[0].items[0].min = 7;
        t.themes[0].items[0].max = 3;
        assert!(t.validate().is_err());
        let mut t = base;
        t.themes[0].items[0].id = "bread".to_string();
        assert!(t.validate().is_err());
    }

    #[test]
    fn custom_table_is_used_and_deterministic() {
        let table = LootTable {
            empty_weight: 0,
            rolls_min: 2,
            rolls_max: 2,
            themes: vec![LootTheme {
                weight: 1,
                items: vec![LootEntry {
                    id: "minecraft:diamond".to_string(),
                    min: 5,
                    max: 5,
                    weight: 1,
                }],
            }],
        };
        let loot = roll_chest(&table, 10, -4, 3);
        assert!(!loot.is_empty());
        for item in &loot {
            assert_eq!(item["id"], Value::String("minecraft:diamond".to_string()));
            assert_eq!(item["count"], Value::Int(5));
        }
        assert_eq!(roll_chest(&table, 10, -4, 3), loot);
    }
}
