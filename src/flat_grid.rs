//! A contiguous row-major 2D grid: one `Vec<T>` plus a stride, replacing the
//! former `Vec<Vec<T>>` storage of the hot per-cell grids (elevation heights,
//! land-cover classification, water distance, water blend).
//!
//! Every indexed access goes through the single `#[inline]` `at`/`set` pair
//! (or the row accessors, which compute the same stride once per row), so the
//! stride arithmetic lives in exactly one place. Iteration order over
//! `rows()`/`as_slice()` is identical to the old outer-then-inner `Vec<Vec>`
//! order: row-major, z outer, x inner. The values stored are the same bits
//! the nested layout stored — flattening changes memory layout only, never a
//! load's value or the arithmetic done on it.

/// Row-major 2D grid backed by one contiguous allocation.
#[derive(Clone, Debug, Default)]
pub struct FlatGrid<T> {
    data: Vec<T>,
    /// Row stride == logical width.
    width: usize,
    height: usize,
}

impl<T: Copy> FlatGrid<T> {
    /// Grid of `width * height` cells, every cell set to `fill`.
    pub fn new(width: usize, height: usize, fill: T) -> Self {
        Self {
            data: vec![fill; width * height],
            width,
            height,
        }
    }

    /// Wrap an existing row-major buffer. `data.len()` must equal `width * height`.
    pub fn from_vec(data: Vec<T>, width: usize, height: usize) -> Self {
        assert_eq!(
            data.len(),
            width * height,
            "FlatGrid::from_vec: buffer length {} != {width} x {height}",
            data.len()
        );
        Self {
            data,
            width,
            height,
        }
    }

    /// Flatten nested rows (the legacy `Vec<Vec<T>>` layout) preserving order.
    /// Every row must have the same length.
    pub fn from_rows(rows: Vec<Vec<T>>) -> Self {
        let height = rows.len();
        let width = rows.first().map_or(0, Vec::len);
        let mut data = Vec::with_capacity(width * height);
        for row in rows {
            assert_eq!(row.len(), width, "FlatGrid::from_rows: ragged rows");
            data.extend_from_slice(&row);
        }
        Self {
            data,
            width,
            height,
        }
    }

    /// Value at row `z`, column `x` (bounds-checked like the old `grid[z][x]`).
    #[inline(always)]
    pub fn at(&self, z: usize, x: usize) -> T {
        debug_assert!(x < self.width);
        self.data[z * self.width + x]
    }

    /// Store `v` at row `z`, column `x`.
    #[inline(always)]
    pub fn set(&mut self, z: usize, x: usize, v: T) {
        debug_assert!(x < self.width);
        self.data[z * self.width + x] = v;
    }

    /// Row `z` as a slice.
    #[inline(always)]
    pub fn row(&self, z: usize) -> &[T] {
        let start = z * self.width;
        &self.data[start..start + self.width]
    }

    /// Row `z` as a mutable slice.
    #[inline(always)]
    pub fn row_mut(&mut self, z: usize) -> &mut [T] {
        let start = z * self.width;
        &mut self.data[start..start + self.width]
    }

    #[inline(always)]
    pub fn width(&self) -> usize {
        self.width
    }

    #[inline(always)]
    pub fn height(&self) -> usize {
        self.height
    }

    /// True when the grid holds no cells.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// The whole buffer, row-major (same element order the nested layout had).
    #[inline(always)]
    pub fn as_slice(&self) -> &[T] {
        &self.data
    }

    /// The whole buffer, mutable, row-major.
    #[inline(always)]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.data
    }

    /// Iterate rows in order, each as a slice — the flat replacement for
    /// `grid.iter()` over the old nested rows.
    #[inline]
    pub fn rows(&self) -> std::slice::ChunksExact<'_, T> {
        // width 0 => empty data; chunk size 1 on an empty slice yields nothing.
        self.data.chunks_exact(self.width.max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_rows() {
        let g = FlatGrid::from_rows(vec![vec![1u8, 2, 3], vec![4, 5, 6]]);
        assert_eq!(g.width(), 3);
        assert_eq!(g.height(), 2);
        assert_eq!(g.at(0, 2), 3);
        assert_eq!(g.at(1, 0), 4);
        assert_eq!(g.row(1), &[4, 5, 6]);
        let rows: Vec<&[u8]> = g.rows().collect();
        assert_eq!(rows, vec![&[1u8, 2, 3][..], &[4, 5, 6][..]]);
    }

    #[test]
    fn set_and_row_mut() {
        let mut g = FlatGrid::new(2, 2, 0u8);
        g.set(1, 1, 9);
        assert_eq!(g.at(1, 1), 9);
        g.row_mut(0).copy_from_slice(&[7, 8]);
        assert_eq!(g.as_slice(), &[7, 8, 0, 9]);
    }

    #[test]
    fn empty_grid_is_safe() {
        let g: FlatGrid<u8> = FlatGrid::from_rows(Vec::new());
        assert!(g.is_empty());
        assert_eq!(g.rows().count(), 0);
    }
}
