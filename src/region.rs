//! Region-aware tree library. Loads a realm's `region.json` (communities grouped by habitat,
//! each community a set of species, each species a set of size-bucketed `.schem` files) plus the
//! sibling `vanilla-plus` pack, and picks a schematic per cell with an 85/12/3 blend:
//! 85% the matched regional community, 12% a vanilla-plus sprinkle, 3% a rare regional exotic.
//!
//! The realm is chosen upstream (Meld picks the pack dir from the selection lat/lon); this module
//! only loads `--tree-pack <realm_dir>` and resolves the vanilla sibling for the sprinkle. Every
//! pick is a pure function of the (slot) coordinate, so it is identical from any tile (seam-safe).

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::land_cover::coord_hash;
use crate::schematic::{load_schem, Schematic};
use crate::tree_library::{size_for_height, TreeSize};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Habitat {
    Conifer,
    Wet,
    Lowland,
    Dry,
    /// Tropical (jungle/palm). `habitat_for_tree_type` never returns this, so a Tropical community
    /// is only ever reachable in a realm that has no other bucket - i.e. the vanilla-plus jungle is
    /// kept OUT of the temperate sprinkle, while genuinely tropical realms use their own lowland
    /// rainforest communities.
    Tropical,
}

impl Habitat {
    fn parse(s: &str) -> Habitat {
        match s {
            "conifer" => Habitat::Conifer,
            "wet" => Habitat::Wet,
            "dry" => Habitat::Dry,
            "tropical" => Habitat::Tropical,
            _ => Habitat::Lowland,
        }
    }
}

// ---- region.json manifest (serde) ----
#[derive(Deserialize)]
struct MSpecies {
    // `name` is present in the JSON but unused at runtime (serde ignores it).
    files: Vec<String>,
    /// Wide-trunk variants of this species, kept but drawn rarely.
    #[serde(default)]
    wide: Vec<String>,
}

/// Percent chance a tree is a wide-trunk variant (when the chosen species has any). Wide trees
/// look heavy, so they stay an occasional accent, not the norm.
const WIDE_PCT: u64 = 12;
#[derive(Deserialize)]
struct MCommunity {
    name: String,
    habitat: String,
    species: Vec<MSpecies>,
}
#[derive(Deserialize)]
struct MRegion {
    realm: String,
    default_community: String,
    communities: Vec<MCommunity>,
}

/// A loaded community: its habitat plus its species, each species holding the global entry
/// indices of its size-bucketed variants.
struct Community {
    name: String,
    habitat: Habitat,
    species: Vec<Vec<usize>>, // per species: entry indices (weight = len)
}

/// A loaded set of communities (one realm pack, or the vanilla-plus sprinkle pack).
struct Pack {
    communities: Vec<Community>,
    default_idx: usize,                      // index of the default (fallback) community
    by_habitat: HashMap<Habitat, Vec<usize>>, // habitat -> community indices
}

impl Pack {
    fn is_empty(&self) -> bool {
        self.communities.is_empty()
    }
}

pub struct RegionLibrary {
    realm: String,
    entries: Vec<(Schematic, TreeSize, bool)>, // schem, size, is_wide (realm + vanilla)
    realm_pack: Pack,
    vanilla_pack: Pack, // sprinkle; may be empty (then the 12% slice falls back to realm)
    scale: f64,
    ground_level: i32, // the world's base Y (selection min elevation maps here) for montane test
    total_realm: usize,
    total_vanilla: usize,
}

/// Metres above the selection's lowest point at which a cell counts as montane (then lowland/wet
/// communities are swapped for conifer/alpine ones). Converted to blocks via the map scale.
const MONTANE_METRES: f64 = 450.0;

/// Load every file of a manifest into `entries`, returning the built `Pack`. Files are resolved
/// relative to `dir`. Empty species/communities are dropped.
fn load_pack(dir: &Path, m: &MRegion, entries: &mut Vec<(Schematic, TreeSize, bool)>) -> Pack {
    let mut communities: Vec<Community> = Vec::new();
    for mc in &m.communities {
        let mut species: Vec<Vec<usize>> = Vec::new();
        for sp in &mc.species {
            let mut idxs: Vec<usize> = Vec::new();
            for (rels, is_wide) in [(&sp.files, false), (&sp.wide, true)] {
                for rel in rels {
                    let path = dir.join(rel);
                    let Ok(bytes) = std::fs::read(&path) else {
                        continue;
                    };
                    if let Ok(schem) = load_schem(&bytes) {
                        if !schem.voxels.is_empty() {
                            let size = size_for_height(schem.height);
                            entries.push((schem, size, is_wide));
                            idxs.push(entries.len() - 1);
                        }
                    }
                }
            }
            if !idxs.is_empty() {
                species.push(idxs);
            }
        }
        if !species.is_empty() {
            communities.push(Community {
                name: mc.name.clone(),
                habitat: Habitat::parse(&mc.habitat),
                species,
            });
        }
    }
    let default_idx = communities
        .iter()
        .position(|c| c.name == m.default_community)
        .or_else(|| communities.iter().position(|c| c.habitat == Habitat::Lowland))
        .unwrap_or(0);
    let mut by_habitat: HashMap<Habitat, Vec<usize>> = HashMap::new();
    for (i, c) in communities.iter().enumerate() {
        by_habitat.entry(c.habitat).or_default().push(i);
    }
    Pack {
        communities,
        default_idx,
        by_habitat,
    }
}

fn read_manifest(dir: &Path) -> Result<MRegion, String> {
    let p = dir.join("region.json");
    let bytes =
        std::fs::read(&p).map_err(|e| format!("region: {}: {e}", p.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("region: parse {}: {e}", p.display()))
}

impl RegionLibrary {
    /// Load the realm pack at `dir` (must contain `region.json`) plus the sibling `vanilla-plus`
    /// pack for the sprinkle. Errors if `dir/region.json` is missing (caller falls back to the
    /// plain vanilla loader).
    pub fn load(dir: &Path, scale: f64, ground_level: i32) -> Result<RegionLibrary, String> {
        let m = read_manifest(dir)?;
        let mut entries: Vec<(Schematic, TreeSize, bool)> = Vec::new();
        let realm_pack = load_pack(dir, &m, &mut entries);
        if realm_pack.is_empty() {
            return Err(format!("region: no usable trees under {}", dir.display()));
        }
        let total_realm = entries.len();

        // Sibling vanilla-plus for the sprinkle (skip when this realm IS vanilla-plus).
        let mut vanilla_pack = Pack {
            communities: Vec::new(),
            default_idx: 0,
            by_habitat: HashMap::new(),
        };
        if m.realm != "vnplus" {
            if let Some(parent) = dir.parent() {
                let vdir = parent.join("vanilla-plus");
                if let Ok(vm) = read_manifest(&vdir) {
                    vanilla_pack = load_pack(&vdir, &vm, &mut entries);
                }
            }
        }
        let total_vanilla = entries.len() - total_realm;

        Ok(RegionLibrary {
            realm: m.realm,
            entries,
            realm_pack,
            vanilla_pack,
            scale,
            ground_level,
            total_realm,
            total_vanilla,
        })
    }

    /// True if `(x, z)`'s terrain Y is high enough above the world baseline to be montane (so
    /// lowland/wet communities are swapped for conifer ones). Pure function of `elev_y`.
    fn is_montane(&self, elev_y: i32) -> bool {
        let blocks_above = f64::from(elev_y - self.ground_level);
        blocks_above / self.scale.max(0.001) > MONTANE_METRES
    }

    pub fn schem(&self, idx: usize) -> &Schematic {
        &self.entries[idx].0
    }

    /// Map scale, for scale-aware trunk spacing at the call site.
    pub fn scale(&self) -> f64 {
        self.scale
    }

    /// The size tier wanted at this cell, by the scale band. Huge only appears at high scale and
    /// stays rare; small maps lean small/medium. Position-seeded (seam-safe).
    fn size_pick(&self, x: i32, z: i32) -> TreeSize {
        let roll = coord_hash(x + 101, z + 233) % 100;
        if self.scale < 0.3 {
            // small ratio: mostly small + medium, big rare, NEVER huge
            if roll < 65 {
                TreeSize::Small
            } else if roll < 98 {
                TreeSize::Medium
            } else {
                TreeSize::Big
            }
        } else if self.scale < 0.7 {
            if roll < 40 {
                TreeSize::Small
            } else if roll < 85 {
                TreeSize::Medium
            } else {
                TreeSize::Big
            }
        } else if self.scale < 1.0 {
            if roll < 28 {
                TreeSize::Small
            } else if roll < 73 {
                TreeSize::Medium
            } else if roll < 95 {
                TreeSize::Big
            } else {
                TreeSize::Huge
            }
        } else if roll < 20 {
            TreeSize::Small
        } else if roll < 65 {
            TreeSize::Medium
        } else if roll < 93 {
            TreeSize::Big
        } else {
            TreeSize::Huge
        }
    }

    /// Whether a size may appear at this scale at all. Huge giants are forbidden below 1:1.4 - the
    /// no-leak rule: a species that only has huge variants is simply skipped on a small map rather
    /// than dropping a giant.
    fn size_allowed(&self, size: TreeSize) -> bool {
        !matches!(size, TreeSize::Huge) || self.scale >= 0.7
    }

    /// Pick one variant entry index from a community: species weighted by their ALLOWED-size variant
    /// count, then the wanted size tier (else any allowed size of that species). Never returns a
    /// disallowed size (so huge never leaks onto a small map).
    fn pick_in_community(&self, c: &Community, x: i32, z: i32) -> Option<usize> {
        let allowed_count =
            |sp: &Vec<usize>| sp.iter().filter(|&&i| self.size_allowed(self.entries[i].1)).count();
        let total: usize = c.species.iter().map(&allowed_count).sum();
        if total == 0 {
            // Community has nothing in an allowed size at this scale: place anything rather than a
            // gap (rare - a tiny all-huge community on a small map).
            let any: usize = c.species.iter().map(Vec::len).sum();
            if any == 0 {
                return None;
            }
            let mut r = (coord_hash(x + 31, z + 57) % any as u64) as usize;
            for sp in &c.species {
                if r < sp.len() {
                    let h = coord_hash(x + 313, z + 727) as usize;
                    return Some(sp[h % sp.len()]);
                }
                r -= sp.len();
            }
            return None;
        }
        // weighted species choice (by allowed-size count)
        let mut r = (coord_hash(x + 31, z + 57) % total as u64) as usize;
        let mut chosen: &Vec<usize> = &c.species[0];
        for sp in &c.species {
            let w = allowed_count(sp);
            if r < w {
                chosen = sp;
                break;
            }
            r -= w;
        }
        let want = self.size_pick(x, z);
        // Allowed-size variants of the chosen species, split normal vs wide-trunk. Wide trees stay
        // an occasional accent (WIDE_PCT), never the norm.
        let allowed: Vec<usize> = chosen
            .iter()
            .copied()
            .filter(|&i| self.size_allowed(self.entries[i].1))
            .collect();
        if allowed.is_empty() {
            return None;
        }
        let wide: Vec<usize> = allowed.iter().copied().filter(|&i| self.entries[i].2).collect();
        let normal: Vec<usize> = allowed.iter().copied().filter(|&i| !self.entries[i].2).collect();
        let use_wide =
            !wide.is_empty() && (normal.is_empty() || coord_hash(x + 5, z + 11) % 100 < WIDE_PCT);
        let group: &[usize] = if use_wide { &wide } else { &normal };
        // within the chosen group, prefer the wanted size tier
        let of_want: Vec<usize> = group
            .iter()
            .copied()
            .filter(|&i| self.entries[i].1 == want)
            .collect();
        let pool: &[usize] = if of_want.is_empty() { group } else { &of_want };
        if pool.is_empty() {
            return None;
        }
        let h = coord_hash(x + 313, z + 727) as usize;
        Some(pool[h % pool.len()])
    }

    /// Choose a community within `pack` for `habitat_hint`: among the communities of that habitat
    /// (a coarse value-noise zone keeps patches coherent), else the pack default forest. On a
    /// montane cell a lowland/wet hint is swapped for conifer, so mountains pull alpine/taiga
    /// communities instead of Mediterranean/beach or jungle ones.
    fn pick_community<'a>(
        &self,
        pack: &'a Pack,
        hint: Habitat,
        x: i32,
        z: i32,
        montane: bool,
    ) -> &'a Community {
        let eff_hint = if montane && matches!(hint, Habitat::Lowland | Habitat::Wet) {
            Habitat::Conifer
        } else {
            hint
        };
        let cand = pack
            .by_habitat
            .get(&eff_hint)
            .filter(|v| !v.is_empty())
            // montane fell through to a realm with no conifer community: keep the lowland set.
            .or_else(|| pack.by_habitat.get(&hint).filter(|v| !v.is_empty()));
        let idx = match cand {
            Some(v) => {
                let n = crate::ground_generation::value_noise_01(x, z, 160);
                let k = ((n * v.len() as f64) as usize).min(v.len() - 1);
                v[k]
            }
            None => pack.default_idx,
        };
        &pack.communities[idx]
    }

    /// Pick a schematic + rotation for the tree at `(x, z)` with the given habitat hint and the
    /// cell's terrain Y (`elev_y`, for montane gating). Applies the 85/12/3 regional /
    /// vanilla-sprinkle / rare-exotic blend. Pure function of `(x, z, elev_y)`.
    pub fn pick(&self, x: i32, z: i32, hint: Habitat, elev_y: i32) -> Option<(usize, u8)> {
        // Bias toward conifer up high, but only in ~60% of zones (a coarse value-noise patch), so
        // mountains keep oak/birch patches instead of turning 100% spruce.
        let montane =
            self.is_montane(elev_y) && crate::ground_generation::value_noise_01(x, z, 64) < 0.6;
        let blend = coord_hash(x + 7, z + 13) % 100;
        let entry = if blend >= 97 {
            // 3% rare: an off-type exotic from a random realm community
            self.pick_rare(x, z)
        } else if blend >= 85 && !self.vanilla_pack.is_empty() {
            // 12% vanilla sprinkle
            let c = self.pick_community(&self.vanilla_pack, hint, x, z, montane);
            self.pick_in_community(c, x, z)
        } else {
            // 85% regional (also covers the vanilla slice when no vanilla sibling is present)
            let c = self.pick_community(&self.realm_pack, hint, x, z, montane);
            self.pick_in_community(c, x, z)
        };
        let idx = entry?;
        let rot = (coord_hash(x ^ 0x5bd1, z ^ 0x9e37) % 4) as u8;
        Some((idx, rot))
    }

    /// A rare exotic: a uniformly chosen realm community (ignores the habitat hint), so unusual
    /// species surface occasionally for variety.
    fn pick_rare(&self, x: i32, z: i32) -> Option<usize> {
        if self.realm_pack.communities.is_empty() {
            return None;
        }
        let ci =
            (coord_hash(x + 5, z + 9) % self.realm_pack.communities.len() as u64) as usize;
        self.pick_in_community(&self.realm_pack.communities[ci], x, z)
    }

    /// Log the loaded breakdown (the numbers Meld surfaces).
    pub fn report(&self) {
        println!(
            "Region tree pack loaded: realm {} - {} regional trees ({} communities) + {} vanilla \
             sprinkle trees ({} communities)",
            self.realm,
            self.total_realm,
            self.realm_pack.communities.len(),
            self.total_vanilla,
            self.vanilla_pack.communities.len(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn habitat_parse() {
        assert_eq!(Habitat::parse("conifer"), Habitat::Conifer);
        assert_eq!(Habitat::parse("wet"), Habitat::Wet);
        assert_eq!(Habitat::parse("dry"), Habitat::Dry);
        assert_eq!(Habitat::parse("tropical"), Habitat::Tropical);
        assert_eq!(Habitat::parse("anything"), Habitat::Lowland);
    }

    #[test]
    #[ignore = "set ARNIS_REGION to a realm dir with region.json"]
    fn smoke_load_real_region() {
        let dir = std::env::var("ARNIS_REGION").expect("ARNIS_REGION");
        let lib = RegionLibrary::load(Path::new(&dir), 1.0, -62).expect("load region");
        lib.report();
        assert!(lib.total_realm > 0);
        // every blend branch resolves to a real entry
        for k in 0..200 {
            if let Some((idx, _)) = lib.pick(k * 3, k * 7, Habitat::Lowland, 0) {
                assert!(idx < lib.entries.len());
            }
        }
    }
}
