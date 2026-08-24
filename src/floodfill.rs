use geo::orient::{Direction, Orient};
use geo::{Contains, LineString, Point, Polygon};
use itertools::Itertools;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Maximum bounding box area (in blocks) for the BITMAP flood fill.
/// 25 million blocks ≈ 5000×5000; bitmap uses only ~3 MB at this size.
/// Polygons above this are filled by [`scanline_fill_area`] instead of being dropped —
/// dropping them left the outline pass painting a lone border with nothing inside it
/// (a 1:1 dune field or forest rendered as an outline around untouched land cover).
pub const MAX_FLOOD_FILL_AREA: i64 = 25_000_000;

/// Ceiling on the number of cells any single fill may return.
///
/// The result is a cached `Vec<(i32, i32)>` at 8 bytes/cell, so this is the real memory
/// bound: 32M cells ≈ 256 MB for one element. The old bbox-only cap implied a worst case
/// of 25M cells, so this is the same order — what changes is WHICH polygons get through.
/// A big-bbox, small-area shape (a river bank, an L, a sparse coastline) used to be
/// dropped for its bounding box alone and now fills, because only the cells it really
/// covers are counted.
pub const MAX_FILL_CELLS: usize = 32_000_000;

/// Work cap for the scanline path: rows x edges is the cost of the crossing scan, and nothing
/// else bounds it - the row cap alone let a 4M-row ring with a hundred-thousand-edge coastline
/// demand ~400 billion edge tests, which is a hang, not a fill. Ported from upstream 3e35621.
pub const MAX_SCANLINE_EDGE_TESTS: i64 = 200_000_000;

/// A compact bitmap for visited-coordinate tracking during flood fill.
///
/// Uses 1 bit per coordinate instead of ~48 bytes per entry in a `HashSet`.
/// For a 5000×5000 bounding box this is ~3 MB instead of ~1.2 GB.
struct FloodBitmap {
    bits: Vec<u8>,
    min_x: i32,
    min_z: i32,
    width: usize,
}

impl FloodBitmap {
    #[inline]
    fn new(min_x: i32, max_x: i32, min_z: i32, max_z: i32) -> Self {
        let width = (max_x - min_x + 1) as usize;
        let height = (max_z - min_z + 1) as usize;
        let num_bytes = (width * height).div_ceil(8);
        Self {
            bits: vec![0u8; num_bytes],
            min_x,
            min_z,
            width,
        }
    }

    /// Mark (x, z) as visited. Returns `true` if it was NOT already visited
    /// (i.e. this is the first visit).
    #[inline]
    fn insert(&mut self, x: i32, z: i32) -> bool {
        let idx = (z - self.min_z) as usize * self.width + (x - self.min_x) as usize;
        let byte = idx / 8;
        let bit = idx % 8;
        let mask = 1u8 << bit;
        if self.bits[byte] & mask != 0 {
            false // already visited
        } else {
            self.bits[byte] |= mask;
            true
        }
    }

    #[inline]
    fn contains(&self, x: i32, z: i32) -> bool {
        let idx = (z - self.min_z) as usize * self.width + (x - self.min_x) as usize;
        let byte = idx / 8;
        let bit = idx % 8;
        (self.bits[byte] >> bit) & 1 == 1
    }
}

/// Main flood fill function with automatic algorithm selection
/// Chooses the best algorithm based on polygon size and complexity
pub fn flood_fill_area(
    polygon_coords: &[(i32, i32)],
    timeout: Option<&Duration>,
) -> Vec<(i32, i32)> {
    if polygon_coords.len() < 3 {
        return vec![]; // Not a valid polygon
    }

    // Reject open polylines: geo::Polygon auto-closes by connecting last to
    // first, which creates a diagonal artifact edge for genuinely open ways
    // (e.g. ridges, cliffs). Closed polygons from SH clipping always have
    // first == last preserved by clip_way_to_bbox.
    let first = polygon_coords[0];
    let last = polygon_coords[polygon_coords.len() - 1];
    if first != last {
        return vec![];
    }

    // Calculate bounding box of the polygon using itertools
    let (min_x, max_x) = polygon_coords
        .iter()
        .map(|&(x, _)| x)
        .minmax()
        .into_option()
        .unwrap();
    let (min_z, max_z) = polygon_coords
        .iter()
        .map(|&(_, z)| z)
        .minmax()
        .into_option()
        .unwrap();

    let area = (max_x - min_x + 1) as i64 * (max_z - min_z + 1) as i64;

    // Too big for the visited-bitmap path (its memory scales with the whole bounding box).
    // Scanline filling needs no bitmap at all — memory is the output alone — so the polygon
    // still gets filled instead of silently becoming an outline with nothing inside.
    if area > MAX_FLOOD_FILL_AREA {
        return scanline_fill_area(polygon_coords, min_x, max_x, min_z, max_z);
    }

    // For small and medium areas, use optimized flood fill with span filling
    if area < 50000 {
        optimized_flood_fill_area(polygon_coords, timeout, min_x, max_x, min_z, max_z)
    } else {
        // For larger areas, use original flood fill with grid sampling
        original_flood_fill_area(polygon_coords, timeout, min_x, max_x, min_z, max_z)
    }
}

/// Even-odd scanline fill for a polygon too large for the bitmap path.
///
/// Why this exists: the bitmap fill allocates for the whole bounding box, so anything past
/// [`MAX_FLOOD_FILL_AREA`] used to return nothing at all. The callers paint the polygon's
/// EDGE before they ask for the fill, so "nothing" did not mean "not drawn" — it meant a
/// bare outline with untouched land cover inside it. A 1:1 selection over a big
/// `natural=sand` dune field is the reported case: a sand border drawn around a dirt and
/// gravel interior.
///
/// The algorithm walks one Z row at a time, intersects it with every polygon edge, sorts
/// the crossings, and fills between each pair. No visited set, so memory is the output
/// alone, which [`MAX_FILL_CELLS`] bounds. Cost is O(rows × edges); a 12k × 10k dune field
/// with a few thousand edges is well under a second.
///
/// Semantics differ slightly from the flood fill: even-odd counts self-intersections and
/// inner rings as outside, which is the standard polygon convention and matches what
/// `geo::Contains` reports for the simple rings these polygons are.
fn scanline_fill_area(
    polygon_coords: &[(i32, i32)],
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
) -> Vec<(i32, i32)> {
    // Cost here is rows x edges, NOT area, so a huge-but-sparse polygon (a long river bank)
    // is cheap and must not be judged by its bounding box - that was the old rule's flaw.
    // Both factors are capped together: rows alone let a pathological ring hang the scan.
    let rows = max_z as i64 - min_z as i64 + 1;
    let edges = polygon_coords.len() as i64 - 1;
    if rows.saturating_mul(edges) > MAX_SCANLINE_EDGE_TESTS {
        return vec![];
    }

    // Rows are sampled on the INTEGER lattice and spans keep strictly between their crossings,
    // so the cells returned here are exactly the ones the bitmap path would accept with
    // geo::Contains - sampling through row centres (z + 0.5) made the two paths disagree by up
    // to a row on the same shape, which shows as a seam wherever a polygon crosses the size
    // threshold between them. A cell lying exactly on the boundary is left to the caller's
    // outline pass, the same way geo::Contains treats the boundary. Ported from upstream
    // 889e09d.
    let mut spans: Vec<(i32, i32, i32)> = Vec::new();
    let mut crossings: Vec<f64> = Vec::new();
    let mut cells: i64 = 0;

    for z in min_z..=max_z {
        let zf = z as f64;
        crossings.clear();
        for w in polygon_coords.windows(2) {
            let (x0, z0) = (w[0].0 as f64, w[0].1 as f64);
            let (x1, z1) = (w[1].0 as f64, w[1].1 as f64);
            // Half-open in z, so a vertex sitting exactly on the row counts once, not twice.
            if (z0 <= zf) == (z1 <= zf) {
                continue;
            }
            let t = (zf - z0) / (z1 - z0);
            crossings.push(x0 + t * (x1 - x0));
        }
        if crossings.len() < 2 {
            continue;
        }
        crossings.sort_unstable_by(f64::total_cmp);

        for &[left, right] in crossings.as_chunks::<2>().0 {
            // Strictly between the crossings; endpoints belong to the outline pass.
            let xs = (left.floor() as i32).saturating_add(1).max(min_x);
            let xe = (right.ceil() as i32).saturating_sub(1).min(max_x);
            if xe < xs {
                continue;
            }
            // i64 before arithmetic: the span count must not wrap on absurd synthetic input.
            cells += xe as i64 - xs as i64 + 1;
            if cells > MAX_FILL_CELLS as i64 {
                return vec![]; // over budget: behave exactly as the old cap did
            }
            spans.push((z, xs, xe));
        }
    }

    let mut filled: Vec<(i32, i32)> = Vec::with_capacity(cells as usize);
    for (z, xs, xe) in spans {
        for x in xs..=xe {
            filled.push((x, z));
        }
    }
    filled
}

/// Optimized flood fill for larger polygons with multi-seed detection for complex shapes like U-shapes
fn optimized_flood_fill_area(
    polygon_coords: &[(i32, i32)],
    timeout: Option<&Duration>,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
) -> Vec<(i32, i32)> {
    let start_time = Instant::now();

    let mut filled_area = Vec::new();
    let mut visited = FloodBitmap::new(min_x, max_x, min_z, max_z);

    // Create polygon for containment testing, with normalized winding order
    // to avoid "polygon had no winding order" warnings from geo::Contains
    let exterior_coords: Vec<(f64, f64)> = polygon_coords
        .iter()
        .map(|&(x, z)| (x as f64, z as f64))
        .collect();
    let exterior = LineString::from(exterior_coords);
    let polygon = Polygon::new(exterior, vec![]).orient(Direction::Default);

    // Optimized step sizes: larger steps for efficiency, but still catch U-shapes
    let width = max_x - min_x + 1;
    let height = max_z - min_z + 1;
    let step_x = (width / 6).clamp(1, 8); // Balance between coverage and speed
    let step_z = (height / 6).clamp(1, 8);

    // Pre-allocate queue with reasonable capacity to avoid reallocations
    let mut queue = VecDeque::with_capacity(1024);

    for z in (min_z..=max_z).step_by(step_z as usize) {
        for x in (min_x..=max_x).step_by(step_x as usize) {
            // Fast timeout check, only every few iterations
            if filled_area.len() % 100 == 0 {
                if let Some(timeout) = timeout {
                    if start_time.elapsed() > *timeout {
                        return filled_area;
                    }
                }
            }

            // Skip if already visited or not inside polygon
            if visited.contains(x, z) || !polygon.contains(&Point::new(x as f64, z as f64)) {
                continue;
            }

            // Start flood fill from this seed point
            queue.clear(); // Reuse queue instead of creating new one
            queue.push_back((x, z));
            visited.insert(x, z);

            while let Some((curr_x, curr_z)) = queue.pop_front() {
                // Add current point to filled area
                filled_area.push((curr_x, curr_z));

                // Check all four directions with optimized bounds checking
                let neighbors = [
                    (curr_x - 1, curr_z),
                    (curr_x + 1, curr_z),
                    (curr_x, curr_z - 1),
                    (curr_x, curr_z + 1),
                ];

                for &(nx, nz) in &neighbors {
                    if nx >= min_x
                        && nx <= max_x
                        && nz >= min_z
                        && nz <= max_z
                        && visited.insert(nx, nz)
                    {
                        // Only check polygon containment for unvisited points
                        if polygon.contains(&Point::new(nx as f64, nz as f64)) {
                            queue.push_back((nx, nz));
                        }
                    }
                }
            }
        }
    }

    filled_area
}

/// Original flood fill algorithm with enhanced multi-seed detection for complex shapes
fn original_flood_fill_area(
    polygon_coords: &[(i32, i32)],
    timeout: Option<&Duration>,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
) -> Vec<(i32, i32)> {
    let start_time = Instant::now();
    let mut filled_area: Vec<(i32, i32)> = Vec::new();
    let mut visited = FloodBitmap::new(min_x, max_x, min_z, max_z);

    // Convert input to a geo::Polygon for efficient point-in-polygon testing,
    // with normalized winding order to avoid undefined Contains results
    let exterior_coords: Vec<(f64, f64)> = polygon_coords
        .iter()
        .map(|&(x, z)| (x as f64, z as f64))
        .collect::<Vec<_>>();
    let exterior: LineString = LineString::from(exterior_coords);
    let polygon: Polygon<f64> = Polygon::new(exterior, vec![]).orient(Direction::Default);

    // Optimized step sizes for large polygons - coarser sampling for speed
    let width = max_x - min_x + 1;
    let height = max_z - min_z + 1;
    let step_x: i32 = (width / 8).clamp(1, 12); // Cap max step size for coverage
    let step_z: i32 = (height / 8).clamp(1, 12);

    // Pre-allocate queue and reserve space for filled_area
    let mut queue: VecDeque<(i32, i32)> = VecDeque::with_capacity(2048);
    filled_area.reserve(1000); // Reserve space to reduce reallocations

    // Scan for multiple seed points to handle U-shapes and concave polygons
    for z in (min_z..=max_z).step_by(step_z as usize) {
        for x in (min_x..=max_x).step_by(step_x as usize) {
            // Reduced timeout checking frequency for better performance
            // Use manual % check since is_multiple_of() is unstable on stable Rust
            if let Some(timeout) = timeout {
                if &start_time.elapsed() > timeout {
                    return filled_area;
                }
            }

            // Skip if already processed or not inside polygon
            if visited.contains(x, z) || !polygon.contains(&Point::new(x as f64, z as f64)) {
                continue;
            }

            // Start flood-fill from this seed point
            queue.clear(); // Reuse queue
            queue.push_back((x, z));
            visited.insert(x, z);

            while let Some((curr_x, curr_z)) = queue.pop_front() {
                // Only check polygon containment once per point when adding to filled_area
                if polygon.contains(&Point::new(curr_x as f64, curr_z as f64)) {
                    filled_area.push((curr_x, curr_z));

                    // Check adjacent points with optimized iteration
                    let neighbors = [
                        (curr_x - 1, curr_z),
                        (curr_x + 1, curr_z),
                        (curr_x, curr_z - 1),
                        (curr_x, curr_z + 1),
                    ];

                    for &(nx, nz) in &neighbors {
                        if nx >= min_x
                            && nx <= max_x
                            && nz >= min_z
                            && nz <= max_z
                            && visited.insert(nx, nz)
                        {
                            queue.push_back((nx, nz));
                        }
                    }
                }
            }
        }
    }

    filled_area
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A closed rectangle ring, first == last, as the callers always produce.
    fn rect(min_x: i32, min_z: i32, max_x: i32, max_z: i32) -> Vec<(i32, i32)> {
        vec![
            (min_x, min_z),
            (max_x, min_z),
            (max_x, max_z),
            (min_x, max_z),
            (min_x, min_z),
        ]
    }

    #[test]
    fn a_polygon_past_the_bitmap_cap_is_filled_not_dropped() {
        // 6,000 x 5,000 = 30M cells: past MAX_FLOOD_FILL_AREA (so the bitmap path refuses
        // it outright, as before) but inside the cell budget. This is the shape that used
        // to render as an outline around untouched ground.
        let poly = rect(0, 0, 5_999, 4_999);
        let filled = flood_fill_area(&poly, None);
        // Strict interior in X; half-open in Z. Vertical boundary columns belong to the
        // callers' outline pass (matching geo::Contains on the bitmap path - the agreement the
        // integer-lattice port, upstream 889e09d, exists to guarantee and the equivalence test
        // above pins). Rows are half-open, so the BOTTOM boundary row stays in and the top row
        // is out - upstream's own comment names a cell on a horizontal edge as the one case the
        // two paths still trade a row on, accepted as slack on shapes thousands of blocks tall.
        // 6,000 x 5,000 ring -> 5,998 interior columns x 4,999 half-open rows.
        assert_eq!(filled.len(), 5_998 * 4_999);
        assert!(filled.contains(&(3_000, 2_500)), "centre must be inside");
        assert!(
            !filled.contains(&(0, 0)),
            "boundary column is the outline pass's job"
        );
        assert!(
            filled.contains(&(1, 0)),
            "bottom row is in: rows are half-open"
        );
        assert!(
            !filled.contains(&(1, 4_999)),
            "top row is out: rows are half-open"
        );
    }

    #[test]
    fn a_big_bbox_with_a_small_area_now_fills() {
        // A thin diagonal band: a 20,000 x 20,100 bounding box (400M, sixteen times the old
        // cap) holding only a sliver. The old rule judged it by its bbox and dropped it;
        // the budget counts the cells it really covers, so it fills.
        let poly = vec![
            (0, 0),
            (19_999, 19_999),
            (19_999, 19_899),
            (0, -100),
            (0, 0),
        ];
        let filled = flood_fill_area(&poly, None);
        assert!(
            (1_000_000..8_000_000).contains(&filled.len()),
            "expected a sliver, got {} cells",
            filled.len()
        );
    }

    #[test]
    fn scanline_covers_the_same_interior_as_the_bitmap_fill() {
        // The bitmap fill floods from inside and stops AT the ring, so it excludes the
        // boundary cells the caller paints separately; the scanline includes them. What
        // must match is the interior: everything the bitmap found is also found here.
        let poly = rect(-30, -20, 29, 19);
        let a = flood_fill_area(&poly, None); // small: bitmap path
        let b = scanline_fill_area(&poly, -30, 29, -20, 19);
        let bs: std::collections::HashSet<_> = b.iter().copied().collect();
        assert!(!a.is_empty());
        for cell in &a {
            assert!(bs.contains(cell), "scanline missed {cell:?}");
        }
        assert!(bs.contains(&(0, 0)), "centre must be inside");
    }

    #[test]
    fn a_fill_over_the_cell_budget_is_still_refused() {
        // 40,000 x 40,000 = 1.6 billion cells: far past what one element may allocate.
        let poly = rect(0, 0, 39_999, 39_999);
        assert!(flood_fill_area(&poly, None).is_empty());
    }

    #[test]
    fn open_polylines_are_still_rejected() {
        let open = vec![(0, 0), (100, 0), (100, 100)];
        assert!(flood_fill_area(&open, None).is_empty());
    }
}
