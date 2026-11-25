//! Tile addressing (XYZ scheme, Web Mercator).

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct TileId {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

impl TileId {
    pub fn parent(&self) -> Option<TileId> {
        if self.z == 0 {
            None
        } else {
            Some(TileId { z: self.z - 1, x: self.x >> 1, y: self.y >> 1 })
        }
    }

    /// Top-left corner and edge length of this tile in normalized Web Mercator
    /// space, where the whole world is the unit square (y grows southwards).
    pub fn origin_size(&self) -> (f64, f64, f64) {
        let n = (1u64 << self.z) as f64;
        (self.x as f64 / n, self.y as f64 / n, 1.0 / n)
    }

    pub fn center_merc(&self) -> (f64, f64) {
        let (ox, oy, s) = self.origin_size();
        (ox + s * 0.5, oy + s * 0.5)
    }
}
