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
use crate::tree_library::{size_for_height, SizeWeights, TreeSize};

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
    // `name` is present in the JSON but unused at runtime (serde ignores it). Variants are split by
    // trunk-width class: w1 = thin (1-wide solid trunk), w2 = 2-wide, w3 = 3-wide. >3 is excluded.
    #[serde(default)]
    w1: Vec<String>,
    #[serde(default)]
    w2: Vec<String>,
    #[serde(default)]
    w3: Vec<String>,
}

/// Cumulative width-class weights (percent): most trees are thin (1-wide), 2-wide occasional,
/// 3-wide rare. So heavy trunks never dominate. W1=78, then +18 to 96 for W2, rest (4) for W3.
const WIDTH_W1: u64 = 78;
const WIDTH_W2: u64 = 96;
fn default_density() -> u32 {
    20
}
#[derive(Deserialize)]
struct MCommunity {
    name: String,
    habitat: String,
    species: Vec<MSpecies>,
    /// WorldPainter Bo2 layer density (~10..150) for this community. Drives how dense/sparse the
    /// scatter is: low = sparse scattered clumps, high = dense thicket. Defaults to 20 (medium).
    #[serde(default = "default_density")]
    density: u32,
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
    density: u32,             // WorldPainter scatter density for this community
}

/// A loaded set of communities (one realm pack, or the vanilla-plus sprinkle pack).
struct Pack {
    communities: Vec<Community>,
    default_idx: usize, // index of the default (fallback) community
    by_habitat: HashMap<Habitat, Vec<usize>>, // habitat -> community indices
}

impl Pack {
    fn is_empty(&self) -> bool {
        self.communities.is_empty()
    }
}

pub struct RegionLibrary {
    realm: String,
    entries: Vec<(Schematic, TreeSize, u8)>, // schem, size, is_wide (realm + vanilla)
    realm_pack: Pack,
    vanilla_pack: Pack, // sprinkle; may be empty (then the 12% slice falls back to realm)
    scale: f64,
    ground_level: i32, // the world's base Y (selection min elevation maps here) for montane test
    weights: SizeWeights, // relative popularity per size tier (the Meld sliders)
    total_realm: usize,
    total_vanilla: usize,
}

/// Metres above the selection's lowest point at which a cell counts as montane (then lowland/wet
/// communities are swapped for conifer/alpine ones). Converted to blocks via the map scale.
const MONTANE_METRES: f64 = 450.0;

/// Load every file of a manifest into `entries`, returning the built `Pack`. Files are resolved
/// relative to `dir`. Empty species/communities are dropped.
fn load_pack(dir: &Path, m: &MRegion, entries: &mut Vec<(Schematic, TreeSize, u8)>) -> Pack {
    let mut communities: Vec<Community> = Vec::new();
    for mc in &m.communities {
        let mut species: Vec<Vec<usize>> = Vec::new();
        for sp in &mc.species {
            let mut idxs: Vec<usize> = Vec::new();
            for (rels, wclass) in [(&sp.w1, 1u8), (&sp.w2, 2u8), (&sp.w3, 3u8)] {
                for rel in rels {
                    let path = dir.join(rel);
                    let Ok(bytes) = std::fs::read(&path) else {
                        continue;
                    };
                    if let Ok(schem) = load_schem(&bytes) {
                        if !schem.voxels.is_empty() {
                            let size = size_for_height(schem.height);
                            entries.push((schem, size, wclass));
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
                density: mc.density,
            });
        }
    }
    let default_idx = communities
        .iter()
        .position(|c| c.name == m.default_community)
        .or_else(|| {
            communities
                .iter()
                .position(|c| c.habitat == Habitat::Lowland)
        })
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
    let bytes = std::fs::read(&p).map_err(|e| format!("region: {}: {e}", p.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("region: parse {}: {e}", p.display()))
}

impl RegionLibrary {
    /// Load the realm pack at `dir` (must contain `region.json`) plus the sibling `vanilla-plus`
    /// pack for the sprinkle. Errors if `dir/region.json` is missing (caller falls back to the
    /// plain vanilla loader).
    pub fn load(
        dir: &Path,
        scale: f64,
        ground_level: i32,
        weights: SizeWeights,
    ) -> Result<RegionLibrary, String> {
        let m = read_manifest(dir)?;
        let mut entries: Vec<(Schematic, TreeSize, u8)> = Vec::new();
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
            weights,
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

    /// The size tier WANTED at this cell, by the scale band. Tall (21-28) is rare and Giant (29+)
    /// ultra-rare and only at 1:1; small maps lean small/medium. If the wanted tier is disabled or
    /// disallowed at this scale, `pick_in_community` falls back to a thinner allowed tier, so a
    /// disabled Giant roll simply becomes a smaller tree (never a gap). Position-seeded (seam-safe).
    /// The default per-tier share (out of 1000) for this scale band, [Small, Medium, Big, Tall,
    /// Giant]. These are exactly the cumulative thresholds the original hardcoded picker used, so
    /// the weighted path multiplies against the true defaults.
    fn base_share_for_scale(&self) -> [f64; 5] {
        if self.scale < 0.3 {
            [650.0, 335.0, 15.0, 0.0, 0.0]
        } else if self.scale < 0.7 {
            [380.0, 440.0, 165.0, 15.0, 0.0]
        } else if self.scale < 1.0 {
            [260.0, 440.0, 230.0, 70.0, 0.0]
        } else {
            [200.0, 400.0, 280.0, 95.0, 25.0]
        }
    }

    fn size_pick(&self, x: i32, z: i32) -> TreeSize {
        let roll = coord_hash(x + 101, z + 233) % 1000;
        // Default sliders: run the ORIGINAL integer-threshold picker so output is byte-identical.
        // (The weighted float path below shifts boundaries once the tier sums differ from the
        // original cumulative layout, so the short-circuit is required, not merely an optimisation.)
        if self.weights.is_default() {
            return if self.scale < 0.3 {
                // tiny maps: small/medium, big rare, never tall/giant
                if roll < 650 {
                    TreeSize::Small
                } else if roll < 985 {
                    TreeSize::Medium
                } else {
                    TreeSize::Big
                }
            } else if self.scale < 0.7 {
                if roll < 380 {
                    TreeSize::Small
                } else if roll < 820 {
                    TreeSize::Medium
                } else if roll < 985 {
                    TreeSize::Big
                } else {
                    TreeSize::Tall // ~1.5%
                }
            } else if self.scale < 1.0 {
                if roll < 260 {
                    TreeSize::Small
                } else if roll < 700 {
                    TreeSize::Medium
                } else if roll < 930 {
                    TreeSize::Big
                } else {
                    TreeSize::Tall // ~7% (no Giant below 1:1)
                }
            } else {
                // 1:1 - all tiers; Tall ~9.5%, Giant ~2.5%
                if roll < 200 {
                    TreeSize::Small
                } else if roll < 600 {
                    TreeSize::Medium
                } else if roll < 880 {
                    TreeSize::Big
                } else if roll < 975 {
                    TreeSize::Tall
                } else {
                    TreeSize::Giant
                }
            };
        }
        // Weighted path (user moved a slider): reweight the default band shares by the per-tier
        // multipliers and bucket the SAME roll, so it stays tile-invariant/seam-safe. A tier the
        // scale band never offered has base share 0 (0*mult = 0), so a slider can't conjure a Giant
        // on a scaled-down map, or a Tall on a tiny map - the depth/scale gates still apply.
        const ORDER: [TreeSize; 5] = [
            TreeSize::Small,
            TreeSize::Medium,
            TreeSize::Big,
            TreeSize::Tall,
            TreeSize::Giant,
        ];
        let base = self.base_share_for_scale();
        let w: [f64; 5] = [
            base[0] * self.weights.small,
            base[1] * self.weights.medium,
            base[2] * self.weights.big,
            base[3] * self.weights.tall,
            base[4] * self.weights.giant,
        ];
        let sum: f64 = w.iter().sum();
        if sum <= 0.0 {
            return TreeSize::Small;
        }
        let target = (roll as f64 / 1000.0) * sum;
        let mut cum = 0.0;
        for (i, weight) in w.iter().enumerate() {
            cum += weight;
            if target < cum {
                return ORDER[i];
            }
        }
        TreeSize::Small
    }

    /// Whether a size may appear, combining the UI tier toggle with a scale gate: Giant (very big)
    /// only renders at 1:1; the other tiers render at any scale they were rolled for. A size that is
    /// toggled off (or Giant below 1:1) is simply skipped - `pick_in_community` falls back to a
    /// thinner allowed tier, so nothing leaks and no gap appears.
    fn size_allowed(&self, size: TreeSize) -> bool {
        if !self.weights.allows(size) {
            return false;
        }
        match size {
            TreeSize::Giant => self.scale >= 1.0,
            _ => true,
        }
    }

    /// Pick one variant entry index from a community: species weighted by their ALLOWED-size variant
    /// count, then the wanted size tier (else any allowed size of that species). Never returns a
    /// disallowed size (so huge never leaks onto a small map).
    fn pick_in_community(&self, c: &Community, x: i32, z: i32) -> Option<usize> {
        let allowed_count = |sp: &Vec<usize>| {
            sp.iter()
                .filter(|&&i| self.size_allowed(self.entries[i].1))
                .count()
        };
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
        // Allowed-size variants of the chosen species.
        let allowed: Vec<usize> = chosen
            .iter()
            .copied()
            .filter(|&i| self.size_allowed(self.entries[i].1))
            .collect();
        if allowed.is_empty() {
            return None;
        }
        // Roll a trunk-width class (mostly thin), then take that class - falling back to a thinner
        // class if empty - so 2/3-wide trunks stay rare and 1-wide dominates.
        let roll = coord_hash(x + 5, z + 11) % 100;
        let target_wc: u8 = if roll < WIDTH_W1 {
            1
        } else if roll < WIDTH_W2 {
            2
        } else {
            3
        };
        let mut group: Vec<usize> = Vec::new();
        for wc in (1..=target_wc).rev() {
            group = allowed
                .iter()
                .copied()
                .filter(|&i| self.entries[i].2 == wc)
                .collect();
            if !group.is_empty() {
                break;
            }
        }
        if group.is_empty() {
            group = allowed;
        }
        // within the chosen group, prefer the wanted size tier
        let of_want: Vec<usize> = group
            .iter()
            .copied()
            .filter(|&i| self.entries[i].1 == want)
            .collect();
        let pool: &[usize] = if of_want.is_empty() { &group } else { &of_want };
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

    /// Base trunk-slot spacing (blocks) by scale. Density then THINS within this grid into clumps
    /// and clearings, so this is the tightest possible spacing, not the average.
    fn base_spacing(&self) -> i32 {
        if self.scale < 0.3 {
            6
        } else if self.scale < 0.7 {
            5
        } else {
            4
        }
    }

    /// WorldPainter community density (~10..150) -> fraction of slots that keep a tree. Low density
    /// (alpine, open woodland) = sparse scattered clumps; high (fan-palm/bamboo thicket, dense
    /// rainforest) = near-continuous canopy. The single biggest knob on the forest's overall look.
    fn keep_prob(density: u32) -> f64 {
        (0.34 + density as f64 / 90.0).clamp(0.30, 1.0)
    }

    /// Coarse value-noise period (blocks) for the grove/clearing pattern. Smooth noise means kept
    /// slots are CONTIGUOUS (real groves with bare clearings between) instead of a uniform grid or
    /// salt-and-pepper thinning.
    const GROVE_PERIOD: i32 = 22;

    /// Pick the trunk SLOT + schematic for a candidate cell `(x, z)`. Returns `(sx, sz, idx, rot)`
    /// where `(sx, sz)` is the slot the tree is stamped on, or `None` if this slot is a clearing /
    /// spacing gap. Spacing is density-aware: the community at the slot sets how many slots keep a
    /// tree, and a coarse grove noise clumps them. Everything is keyed on the SLOT, so every Arnis
    /// spawn inside the cell collapses to the same decision (idempotent, seam-safe).
    pub fn pick_slot(
        &self,
        x: i32,
        z: i32,
        hint: Habitat,
        elev_y: i32,
    ) -> Option<(i32, i32, usize, u8)> {
        let s = self.base_spacing();
        let (sx, sz) = crate::schematic::trunk_slot_s(x, z, s);
        let montane =
            self.is_montane(elev_y) && crate::ground_generation::value_noise_01(sx, sz, 64) < 0.6;
        let blend = coord_hash(sx + 7, sz + 13) % 100;
        let (community, idx): (&Community, Option<usize>) = if blend >= 97 {
            // 3% rare exotic: a uniformly chosen realm community
            let n = self.realm_pack.communities.len();
            if n == 0 {
                return None;
            }
            let ci = (coord_hash(sx + 5, sz + 9) % n as u64) as usize;
            let c = &self.realm_pack.communities[ci];
            (c, self.pick_in_community(c, sx, sz))
        } else if blend >= 67 && !self.vanilla_pack.is_empty() {
            let c = self.pick_community(&self.vanilla_pack, hint, sx, sz, montane);
            (c, self.pick_in_community(c, sx, sz))
        } else {
            let c = self.pick_community(&self.realm_pack, hint, sx, sz, montane);
            (c, self.pick_in_community(c, sx, sz))
        };
        // Grove/clearing gate: smooth grove noise (+ a little per-cell jitter to soften edges)
        // against the community's density-derived keep fraction. Below the threshold = keep a tree.
        let grove = crate::ground_generation::value_noise_01(sx, sz, Self::GROVE_PERIOD);
        let jitter = (coord_hash(sx ^ 0x71c3, sz ^ 0x2d9b) % 1000) as f64 / 1000.0;
        if grove * 0.82 + jitter * 0.18 >= Self::keep_prob(community.density) {
            return None;
        }
        let idx = idx?;
        let rot = (coord_hash(sx ^ 0x5bd1, sz ^ 0x9e37) % 4) as u8;
        Some((sx, sz, idx, rot))
    }

    /// Log the loaded breakdown (the numbers Meld surfaces), including the per-tier schem counts and
    /// which size tiers are enabled.
    pub fn report(&self) {
        let (mut s, mut m, mut b, mut t, mut g) = (0u32, 0u32, 0u32, 0u32, 0u32);
        for (_, size, _) in &self.entries {
            match size {
                TreeSize::Small => s += 1,
                TreeSize::Medium => m += 1,
                TreeSize::Big => b += 1,
                TreeSize::Tall => t += 1,
                TreeSize::Giant => g += 1,
            }
        }
        let pct = |v: f64| format!("{}%", (v * 100.0).round() as i64);
        println!(
            "Region tree pack loaded: realm {} - {} regional trees ({} communities) + {} vanilla \
             sprinkle trees ({} communities)",
            self.realm,
            self.total_realm,
            self.realm_pack.communities.len(),
            self.total_vanilla,
            self.vanilla_pack.communities.len(),
        );
        println!(
            "  size tiers [weight]: small {} [{}], medium {} [{}], big {} [{}], tall {} [{}], giant {} [{}]",
            s, pct(self.weights.small),
            m, pct(self.weights.medium),
            b, pct(self.weights.big),
            t, pct(self.weights.tall),
            g, pct(self.weights.giant),
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
        let lib = RegionLibrary::load(Path::new(&dir), 1.0, -62, SizeWeights::default())
            .expect("load region");
        lib.report();
        assert!(lib.total_realm > 0);
        // every blend branch resolves to a real entry
        for k in 0..200 {
            if let Some((_, _, idx, _)) = lib.pick_slot(k * 3, k * 7, Habitat::Lowland, 0) {
                assert!(idx < lib.entries.len());
            }
        }
    }
}
