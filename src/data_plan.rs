//! Sizes of the data a run fetches, estimated from what is on disk.
// The window (a later wave) is the caller of these estimates.
#![allow(dead_code)]

// Bake sizes, as multiples of the .pbf they come from. Measured on Geofabrik extracts.

/// A Region Download bake of a whole extract: Liechtenstein 3.46 MB to a
/// 6.87 MB .json.zst. A selection bakes its share of the extract's area.
const REGION_BAKE_PER_PBF: f64 = 2.0;
/// Peak memory of a Region Download bake: 4-5x (Romania 330 MB to 1.3-1.6 GB).
const BAKE_RAM_PER_PBF: f64 = 4.5;

/// An arnis-tiles archive: Switzerland 0.58, Liechtenstein 0.63, Austria 0.67, Romania 1.39.
pub const ARCHIVE_PER_PBF: f64 = 0.7;
/// arnis-tiles' chunk store while it bakes: Switzerland 0.81, Austria 0.92, Romania 1.73.
pub const STORE_PER_PBF: f64 = 0.9;

fn times(bytes: u64, ratio: f64) -> u64 {
    (bytes as f64 * ratio).round() as u64
}

/// The Region Download bake of `share` (0-1) of an extract of `pbf` bytes.
pub fn region_bake_bytes(pbf: u64, share: f64) -> u64 {
    times(pbf, REGION_BAKE_PER_PBF * share.clamp(0.0, 1.0))
}

/// The archive arnis-tiles bakes from an extract of `pbf` bytes.
pub fn archive_bytes(pbf: u64) -> u64 {
    times(pbf, ARCHIVE_PER_PBF)
}

/// Peak memory of baking an extract of `pbf` bytes.
pub fn bake_ram_bytes(pbf: u64) -> u64 {
    times(pbf, BAKE_RAM_PER_PBF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bake_sizes_scale_with_the_extract() {
        assert_eq!(region_bake_bytes(3_463_268, 1.0), 6_926_536);
        assert_eq!(region_bake_bytes(1_000, 0.25), 500);
        assert_eq!(region_bake_bytes(1_000, 7.0), region_bake_bytes(1_000, 1.0));
        assert_eq!(bake_ram_bytes(330_000_000), 1_485_000_000);
        assert_eq!(archive_bytes(1_000_000), 700_000);
    }
}
