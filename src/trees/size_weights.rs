//! Relative popularity of the schematic tree size tiers (`--tree-size-weights`).
//!
//! The weights multiply the scale band's default share per tier and bucket the
//! same position-seeded roll, so a weighted pick is still identical from any
//! tile. Left at their defaults they reproduce the band thresholds exactly.

use crate::trees::tree_library::{SizeFilter, TreeSize};

const ORDER: [TreeSize; 5] = [
    TreeSize::Small,
    TreeSize::Medium,
    TreeSize::Big,
    TreeSize::Tall,
    TreeSize::Giant,
];

/// Multiplier per tier, smallest to largest: 1.0 keeps the default share, 0.0 turns the tier off.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SizeWeights([f64; 5]);

impl Default for SizeWeights {
    fn default() -> Self {
        SizeWeights([1.0; 5])
    }
}

impl SizeWeights {
    /// Parse `name=percent` pairs (small, medium, big, tall, giant), each 0-200.
    /// Omitted tiers stay at 100.
    pub fn parse(spec: &str) -> Result<SizeWeights, String> {
        let mut w = SizeWeights::default();
        for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (name, val) = part
                .split_once('=')
                .ok_or_else(|| format!("expected name=percent, got '{part}'"))?;
            let pct: f64 = val
                .trim()
                .parse()
                .map_err(|_| format!("bad percent '{}' for '{}'", val.trim(), name.trim()))?;
            if !(0.0..=200.0).contains(&pct) {
                return Err(format!("percent for '{}' must be 0-200", name.trim()));
            }
            let i = ORDER
                .iter()
                .position(|s| format!("{s:?}").eq_ignore_ascii_case(name.trim()))
                .ok_or_else(|| format!("unknown tree size '{}'", name.trim()))?;
            w.0[i] = pct / 100.0;
        }
        if w.0.iter().all(|&f| f == 0.0) {
            return Err("at least one tree size needs a weight above 0".to_string());
        }
        Ok(w)
    }

    /// A tier weighted 0 is off, so a canopy hint cannot bring it back either.
    pub fn restrict(&self, sizes: &mut SizeFilter) {
        let [s, m, b, t, g] = self.0.map(|f| f > 0.0);
        sizes.small &= s;
        sizes.medium &= m;
        sizes.big &= b;
        sizes.tall &= t;
        sizes.giant &= g;
    }
}

/// Default share per tier out of 1000 for a scale band. Tall rare, Giant only at 1:1.
fn base_shares(scale: f64) -> [u32; 5] {
    if scale < 0.3 {
        [650, 335, 15, 0, 0]
    } else if scale < 0.7 {
        [380, 440, 165, 15, 0]
    } else if scale < 1.0 {
        [260, 440, 230, 70, 0]
    } else {
        [200, 400, 280, 95, 25]
    }
}

/// The size tier for `roll` in 0..1000. A tier the band never offers stays out
/// whatever its weight, so the scale gates still hold.
pub fn pick(roll: u64, scale: f64, weights: Option<&SizeWeights>) -> TreeSize {
    let w = weights.copied().unwrap_or_default();
    let shares: Vec<f64> = base_shares(scale)
        .iter()
        .zip(w.0)
        .map(|(&s, f)| f64::from(s) * f)
        .collect();
    let sum: f64 = shares.iter().sum();
    // Exact for the defaults: every share is a whole number and the sum is 1000.
    let target = roll as f64 * sum / 1000.0;
    let mut cum = 0.0;
    for (size, share) in ORDER.iter().zip(&shares) {
        cum += share;
        if target < cum {
            return *size;
        }
    }
    // Every tier this band offers is weighted 0: the size filter decides.
    TreeSize::Small
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pairs() {
        let w = SizeWeights::parse("big=70, Tall=50,giant=0").unwrap();
        assert_eq!(w.0, [1.0, 1.0, 0.7, 0.5, 0.0]);
        assert!(SizeWeights::parse("huge=10").is_err());
        assert!(SizeWeights::parse("big").is_err());
        assert!(SizeWeights::parse("big=250").is_err());
        assert!(SizeWeights::parse("small=0,medium=0,big=0,tall=0,giant=0").is_err());
    }

    #[test]
    fn zero_weight_turns_the_tier_off() {
        let mut f = SizeFilter::up_to(TreeSize::Tall);
        SizeWeights::parse("medium=0").unwrap().restrict(&mut f);
        assert!(f.small && !f.medium && f.big && f.tall && !f.giant);
    }

    // The unweighted pick must match the band thresholds region.rs always used.
    #[test]
    fn defaults_reproduce_the_band_thresholds() {
        let old = |scale: f64, roll: u64| {
            let cuts: &[u64] = if scale < 0.3 {
                &[650, 985, 1000]
            } else if scale < 0.7 {
                &[380, 820, 985, 1000]
            } else if scale < 1.0 {
                &[260, 700, 930, 1000]
            } else {
                &[200, 600, 880, 975, 1000]
            };
            ORDER[cuts.iter().position(|&c| roll < c).unwrap()]
        };
        let d = SizeWeights::default();
        for scale in [0.1, 0.5, 0.8, 1.0] {
            for roll in 0..1000 {
                assert_eq!(pick(roll, scale, None), old(scale, roll));
                assert_eq!(pick(roll, scale, Some(&d)), old(scale, roll));
            }
        }
    }

    #[test]
    fn weights_shift_the_mix_within_the_band() {
        let count = |w: &SizeWeights, scale: f64, size: TreeSize| {
            (0..1000)
                .filter(|&r| pick(r, scale, Some(w)) == size)
                .count()
        };
        let d = SizeWeights::default();
        let big = SizeWeights::parse("small=0,big=200").unwrap();
        assert_eq!(count(&big, 1.0, TreeSize::Small), 0);
        assert!(count(&big, 1.0, TreeSize::Big) > count(&d, 1.0, TreeSize::Big));
        // A band without giants never grows one, however hard it is asked.
        let giant = SizeWeights::parse("giant=200").unwrap();
        assert_eq!(count(&giant, 0.8, TreeSize::Giant), 0);
    }
}
