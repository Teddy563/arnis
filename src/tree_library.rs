#![allow(dead_code)] // entries/index/candidates consumed by tree placement in a later step.
//! Tree schematic library. Loads a `--tree-pack` directory of Sponge `.schem` files
//! (grouped by species sub-folder), buckets each tree by height into small/medium/big,
//! and applies the curation discard list (pale garden + giant mushrooms only).
//! Exposes a total + size breakdown for the UI.

use std::collections::HashMap;
use std::path::Path;

use crate::schematic::{load_schem, Schematic};

/// Height (block) cut-offs for the size tiers.
const SMALL_MAX_HEIGHT: i32 = 6;
const MEDIUM_MAX_HEIGHT: i32 = 12;
const BIG_MAX_HEIGHT: i32 = 20;
const TALL_MAX_HEIGHT: i32 = 28;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TreeSize {
    Small,
    Medium,
    Big,
    /// 21-28 blocks - tall canopy trees. Kept rare and (by default) only at high scale.
    Tall,
    /// 29+ blocks - the giants. Off by default; only at 1:1 and ultra-rare when enabled. The truly
    /// absurd ones (H>40, up to 117 blocks) are dropped at pack-build, so Giant here means ~29-40.
    Giant,
}

/// Bucket a schematic by its height.
pub fn size_for_height(height: i32) -> TreeSize {
    if height <= SMALL_MAX_HEIGHT {
        TreeSize::Small
    } else if height <= MEDIUM_MAX_HEIGHT {
        TreeSize::Medium
    } else if height <= BIG_MAX_HEIGHT {
        TreeSize::Big
    } else if height <= TALL_MAX_HEIGHT {
        TreeSize::Tall
    } else {
        TreeSize::Giant
    }
}

/// The five size tiers + which are enabled (the Meld UI checkboxes). Default: all but Giant.
#[derive(Clone, Copy, Debug)]
pub struct SizeFilter {
    pub small: bool,
    pub medium: bool,
    pub big: bool,
    pub tall: bool,
    pub giant: bool,
}

impl Default for SizeFilter {
    fn default() -> Self {
        SizeFilter {
            small: true,
            medium: true,
            big: true,
            tall: true,
            giant: false,
        }
    }
}

impl SizeFilter {
    /// True if `size` is ticked on in the UI.
    pub fn allows(&self, size: TreeSize) -> bool {
        match size {
            TreeSize::Small => self.small,
            TreeSize::Medium => self.medium,
            TreeSize::Big => self.big,
            TreeSize::Tall => self.tall,
            TreeSize::Giant => self.giant,
        }
    }

    /// Parse a comma list of enabled tiers (e.g. "small,medium,big,tall"). Unknown tokens ignored.
    /// An empty / all-unknown list falls back to the default (so a tree never silently vanishes).
    pub fn parse(list: &str) -> SizeFilter {
        let mut f = SizeFilter {
            small: false,
            medium: false,
            big: false,
            tall: false,
            giant: false,
        };
        let mut any = false;
        for tok in list.split(',') {
            match tok.trim().to_ascii_lowercase().as_str() {
                "small" | "s" => { f.small = true; any = true; }
                "medium" | "m" => { f.medium = true; any = true; }
                "big" | "b" => { f.big = true; any = true; }
                "tall" | "t" => { f.tall = true; any = true; }
                "giant" | "g" | "huge" => { f.giant = true; any = true; }
                _ => {}
            }
        }
        if any {
            f
        } else {
            SizeFilter::default()
        }
    }
}

/// Choose a size tier from the scale band (medium common at full scale; small-only on very
/// small maps). Position-seeded so it is identical from any tile.
pub fn pick_size(px: i32, pz: i32, scale: f64) -> TreeSize {
    let roll = crate::land_cover::coord_hash(px + 101, pz + 233) % 100;
    if scale >= 0.5 {
        if roll < 20 {
            TreeSize::Small
        } else if roll < 80 {
            TreeSize::Medium
        } else {
            TreeSize::Big
        }
    } else if scale >= 0.2 {
        if roll < 60 {
            TreeSize::Small
        } else {
            TreeSize::Medium
        }
    } else {
        TreeSize::Small
    }
}

pub struct TreeEntry {
    pub species: String,
    pub size: TreeSize,
    pub schem: Schematic,
}

/// Counts surfaced to the UI: total plus the small/medium/big/tall/giant breakdown and per-species.
pub struct LibraryStats {
    pub total: usize,
    pub small: usize,
    pub medium: usize,
    pub big: usize,
    pub tall: usize,
    pub giant: usize,
    pub by_species: Vec<(String, usize)>, // sorted by species name
}

pub struct TreeLibrary {
    pub entries: Vec<TreeEntry>,
    pub stats: LibraryStats,
    index: HashMap<(String, TreeSize), Vec<usize>>,
}

/// True if a (folder, file stem) should be skipped by curation. Drops ONLY the pale garden
/// (pale-oak trees) and giant mushrooms; everything else is used, including the tall pines
/// (the earlier `pinus` exclusion was reverted so high-elevation conifers are available).
fn is_discarded(folder: &str, _file_stem: &str) -> bool {
    let f = folder.to_ascii_lowercase();
    f.contains("pale") // pale garden (pale oak)
        || f.contains("mushroom") // giant mushrooms
}

/// Normalise a species folder name to a stable key.
fn species_key(folder: &str) -> String {
    match folder.trim().to_ascii_lowercase().as_str() {
        "taiga" => "spruce".to_string(), // taiga = conifers (Picea + Pinus) -> spruce bucket
        "swamp" => "swamp_oak".to_string(),
        other => other.replace(' ', "_"),
    }
}

fn compute_stats(entries: &[TreeEntry]) -> LibraryStats {
    let mut small = 0;
    let mut medium = 0;
    let mut big = 0;
    let mut tall = 0;
    let mut giant = 0;
    let mut per: HashMap<String, usize> = HashMap::new();
    for e in entries {
        match e.size {
            TreeSize::Small => small += 1,
            TreeSize::Medium => medium += 1,
            TreeSize::Big => big += 1,
            TreeSize::Tall => tall += 1,
            TreeSize::Giant => giant += 1,
        }
        *per.entry(e.species.clone()).or_default() += 1;
    }
    let mut by_species: Vec<(String, usize)> = per.into_iter().collect();
    by_species.sort();
    LibraryStats {
        total: entries.len(),
        small,
        medium,
        big,
        tall,
        giant,
        by_species,
    }
}

impl TreeLibrary {
    /// Load every `.schem` under `dir/<species>/`, applying curation + size buckets.
    pub fn load(dir: &Path) -> Result<TreeLibrary, String> {
        let mut entries: Vec<TreeEntry> = Vec::new();
        let read =
            std::fs::read_dir(dir).map_err(|e| format!("tree-pack: {}: {e}", dir.display()))?;
        for sub in read.flatten() {
            let sub_path = sub.path();
            if !sub_path.is_dir() {
                continue;
            }
            let folder = sub.file_name().to_string_lossy().to_string();
            let Ok(files) = std::fs::read_dir(&sub_path) else {
                continue;
            };
            for file in files.flatten() {
                let fp = file.path();
                if fp.extension().and_then(|e| e.to_str()) != Some("schem") {
                    continue;
                }
                let stem = fp
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                if is_discarded(&folder, &stem) {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&fp) else {
                    continue;
                };
                match load_schem(&bytes) {
                    Ok(schem) if !schem.voxels.is_empty() => {
                        let size = size_for_height(schem.height);
                        entries.push(TreeEntry {
                            species: species_key(&folder),
                            size,
                            schem,
                        });
                    }
                    Ok(_) => {}
                    Err(e) => eprintln!("tree-pack: skipping {}: {e}", fp.display()),
                }
            }
        }
        if entries.is_empty() {
            return Err(format!(
                "tree-pack: no usable .schem under {}",
                dir.display()
            ));
        }
        let stats = compute_stats(&entries);
        let mut index: HashMap<(String, TreeSize), Vec<usize>> = HashMap::new();
        for (i, e) in entries.iter().enumerate() {
            index
                .entry((e.species.clone(), e.size))
                .or_default()
                .push(i);
        }
        Ok(TreeLibrary {
            entries,
            stats,
            index,
        })
    }

    /// Candidate entry indices for a (species, size); empty slice if none.
    pub fn candidates(&self, species: &str, size: TreeSize) -> &[usize] {
        self.index
            .get(&(species.to_string(), size))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Pick a library entry index for (species, size), falling back across sizes then to oak.
    /// Position-seeded so the same spot always picks the same tree (seam-safe).
    pub fn pick_variant(&self, species: &str, size: TreeSize, px: i32, pz: i32) -> Option<usize> {
        let h = crate::land_cover::coord_hash(px + 313, pz + 727) as usize;
        let order = [
            size,
            TreeSize::Medium,
            TreeSize::Small,
            TreeSize::Big,
            TreeSize::Tall,
            TreeSize::Giant,
        ];
        for sp in [species, "oak"] {
            for &sz in &order {
                let c = self.candidates(sp, sz);
                if !c.is_empty() {
                    return Some(c[h % c.len()]);
                }
            }
        }
        None
    }

    /// Log the total + breakdown (the numbers the UI surfaces).
    pub fn report(&self) {
        let s = &self.stats;
        let by: Vec<String> = s
            .by_species
            .iter()
            .map(|(k, n)| format!("{k} {n}"))
            .collect();
        println!(
            "Tree pack loaded: {} trees (small {}, medium {}, big {}, tall {}, giant {}) across {} species: {}",
            s.total,
            s.small,
            s.medium,
            s.big,
            s.tall,
            s.giant,
            s.by_species.len(),
            by.join(", ")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_buckets() {
        assert_eq!(size_for_height(3), TreeSize::Small);
        assert_eq!(size_for_height(6), TreeSize::Small);
        assert_eq!(size_for_height(7), TreeSize::Medium);
        assert_eq!(size_for_height(12), TreeSize::Medium);
        assert_eq!(size_for_height(13), TreeSize::Big);
        assert_eq!(size_for_height(20), TreeSize::Big);
        assert_eq!(size_for_height(21), TreeSize::Tall);
        assert_eq!(size_for_height(28), TreeSize::Tall);
        assert_eq!(size_for_height(35), TreeSize::Giant);
    }

    #[test]
    fn curation_discards() {
        assert!(is_discarded("pale oak", "Quercus_alba1"));
        assert!(!is_discarded("taiga", "Pinus_piceoides3")); // tall pine now KEPT
        assert!(!is_discarded("taiga", "Picea_generica1")); // spruce kept
        assert!(!is_discarded("oak", "Quercus_generica1"));
        assert!(is_discarded("giant mushrooms", "x"));
    }

    #[test]
    fn species_keys() {
        assert_eq!(species_key("taiga"), "spruce");
        assert_eq!(species_key("swamp"), "swamp_oak");
        assert_eq!(species_key("dark oak"), "dark_oak");
        assert_eq!(species_key("Oak"), "oak");
    }

    #[test]
    #[ignore = "set ARNIS_TREEPACK to the vanilla-plus dir"]
    fn smoke_load_real_pack() {
        let dir = std::env::var("ARNIS_TREEPACK").expect("ARNIS_TREEPACK");
        let lib = TreeLibrary::load(Path::new(&dir)).expect("load pack");
        lib.report();
        assert!(lib.stats.total > 0);
        assert!(lib.candidates("pale_oak", TreeSize::Big).is_empty());
        assert!(!lib.candidates("spruce", TreeSize::Medium).is_empty());
    }
}
