//! The small globe in the corner: it turns to face the place the main view is looking at, keeps the
//! same orientation as the map, and shows a pin where you are. Land and sea are drawn from a small
//! land/water bitmap (Natural Earth 110m land, public domain, baked into `assets/land.bin` by
//! `tools/make_land.py`): each pixel of the disc is traced back to a spot on the planet.

use eframe::egui::{self, Color32, Pos2, Stroke};

use crate::camera::{cross, dot, lonlat_to_vec, normalize, vec_to_lonlat, V3};

const LAND_W: usize = 720;
const LAND_H: usize = 360;

/// A 0.5 degree land/water bitmap of the world.
pub struct Land {
    bits: Vec<u8>,
}

impl Land {
    pub fn load() -> Land {
        Land { bits: include_bytes!("../assets/land.bin").to_vec() }
    }

    fn cell(&self, x: usize, y: usize) -> f32 {
        let i = y.min(LAND_H - 1) * (LAND_W / 8) + (x % LAND_W) / 8;
        match self.bits.get(i) {
            Some(b) => ((b >> (7 - (x % LAND_W) % 8)) & 1) as f32,
            None => 0.0,
        }
    }

    /// Land fraction (0 sea .. 1 land) at a place, smoothed between neighbouring cells.
    pub fn at(&self, lon: f64, lat: f64) -> f32 {
        let fx = (lon + std::f64::consts::PI) / (2.0 * std::f64::consts::PI) * LAND_W as f64 - 0.5;
        let fy = ((std::f64::consts::FRAC_PI_2 - lat) / std::f64::consts::PI * LAND_H as f64 - 0.5).clamp(0.0, LAND_H as f64 - 1.0);
        let x0 = fx.floor();
        let (tx, ty) = ((fx - x0) as f32, (fy - fy.floor()) as f32);
        let xi = x0.rem_euclid(LAND_W as f64) as usize;
        let yi = fy.floor() as usize;
        let a = self.cell(xi, yi) * (1.0 - tx) + self.cell(xi + 1, yi) * tx;
        let b = self.cell(xi, yi + 1) * (1.0 - tx) + self.cell(xi + 1, yi + 1) * tx;
        a * (1.0 - ty) + b * ty
    }

    pub fn is_empty(&self) -> bool {
        self.bits.len() < LAND_W / 8 * LAND_H
    }

    /// The globe as an image `dim` x `dim` pixels, centred on (`lon`, `lat`) and turned by `heading`.
    /// Pixels outside the disc are transparent.
    pub fn render(&self, dim: usize, lon: f64, lat: f64, heading: f64) -> egui::ColorImage {
        let f = Frame::new(lon, lat, heading);
        let mut img = egui::ColorImage::filled([dim, dim], Color32::TRANSPARENT);
        let half = dim as f32 * 0.5;
        let (s, c) = heading.sin_cos();
        let sea = [26.0_f32, 74.0, 138.0];
        let land = [120.0_f32, 168.0, 92.0];
        let ice = [236.0_f32, 242.0, 246.0];
        for py in 0..dim {
            for px in 0..dim {
                // position on the disc, in radii (x right, y down)
                let sx = (px as f32 + 0.5 - half) / half;
                let sy = (py as f32 + 0.5 - half) / half;
                let d2 = sx * sx + sy * sy;
                if d2 >= 1.0 {
                    continue;
                }
                let z = (1.0 - d2).sqrt() as f64;
                // undo the heading turn (a reflection, so it is its own inverse)
                let (sxd, syd) = (sx as f64, sy as f64);
                let x = c * sxd - s * syd;
                let y = -s * sxd - c * syd;
                let p = [
                    x * f.east[0] + y * f.north[0] + z * f.n[0],
                    x * f.east[1] + y * f.north[1] + z * f.n[1],
                    x * f.east[2] + y * f.north[2] + z * f.n[2],
                ];
                let (plon, plat) = vec_to_lonlat(p);
                let frac = self.at(plon, plat);
                let t = (frac * 1.6 - 0.3).clamp(0.0, 1.0);
                let t = t * t * (3.0 - 2.0 * t);
                let polar = (plat.abs().to_degrees() as f32 - 66.0).clamp(0.0, 8.0) / 8.0;
                let on_land = [
                    land[0] + (ice[0] - land[0]) * polar,
                    land[1] + (ice[1] - land[1]) * polar,
                    land[2] + (ice[2] - land[2]) * polar,
                ];
                // light from the upper left, a little darker towards the rim
                let shade = 0.62 + 0.38 * (z as f32) + 0.08 * (-sx - sy).clamp(-1.0, 1.0);
                let rgb: Vec<u8> = (0..3)
                    .map(|k| ((sea[k] + (on_land[k] - sea[k]) * t) * shade).clamp(0.0, 255.0) as u8)
                    .collect();
                // soft edge of the disc
                let edge = ((1.0 - d2.sqrt()) * half).clamp(0.0, 1.0);
                img[(px, py)] = Color32::from_rgba_unmultiplied(rgb[0], rgb[1], rgb[2], (edge * 255.0) as u8);
            }
        }
        img
    }
}

/// Where a world point lands on the globe disc, in units of the radius (x right, y down), and
/// whether it is on the visible side. `h` is the heading of the main view.
pub struct Frame {
    n: V3,
    east: V3,
    north: V3,
    ex: (f64, f64),
    ny: (f64, f64),
}

impl Frame {
    pub fn new(lon: f64, lat: f64, heading: f64) -> Frame {
        let n = lonlat_to_vec(lon, lat);
        let east = normalize(cross([0.0, 1.0, 0.0], n));
        let north = cross(n, east);
        let (s, c) = heading.sin_cos();
        // screen direction of east and of north (y down), as in the compass
        Frame { n, east, north, ex: (c, -s), ny: (-s, -c) }
    }

    pub fn project(&self, p: V3) -> ([f32; 2], bool) {
        let (x, y) = (dot(p, self.east), dot(p, self.north));
        let sx = x * self.ex.0 + y * self.ny.0;
        let sy = x * self.ex.1 + y * self.ny.1;
        ([sx as f32, sy as f32], dot(p, self.n) > 0.0)
    }
}

/// Draws the globe centred at `c` with radius `r` points: the picture made by `Land::render` (which
/// covers `r + 1` points each way), a faint graticule, the rim, a mark at north and a pin in the middle.
pub fn draw(painter: &egui::Painter, c: Pos2, r: f32, tex: Option<&egui::TextureHandle>, lon: f64, lat: f64, heading: f64) {
    let f = Frame::new(lon, lat, heading);
    match tex {
        Some(t) => {
            let rect = egui::Rect::from_center_size(c, egui::vec2(2.0 * (r + 1.0), 2.0 * (r + 1.0)));
            painter.image(t.id(), rect, egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
        None => {
            painter.circle_filled(c, r, Color32::from_rgb(26, 74, 138));
        }
    }
    let at = |p: [f32; 2]| Pos2::new(c.x + p[0] * r, c.y + p[1] * r);

    // graticule every 30 degrees
    let grid = Stroke::new(0.6_f32, Color32::from_white_alpha(46));
    let strip = |pts: &mut Vec<Pos2>, stroke: Stroke| {
        if pts.len() >= 2 {
            painter.add(egui::Shape::line(std::mem::take(pts), stroke));
        } else {
            pts.clear();
        }
    };
    let mut pts: Vec<Pos2> = Vec::new();
    for m in 0..12 {
        let lo = (m as f64 * 30.0).to_radians();
        for k in 0..=36 {
            let la = (-90.0 + k as f64 * 5.0).to_radians();
            let (p, front) = f.project(lonlat_to_vec(lo, la));
            if front {
                pts.push(at(p));
            } else {
                strip(&mut pts, grid);
            }
        }
        strip(&mut pts, grid);
    }
    for pl in [-60.0, -30.0, 0.0, 30.0, 60.0] {
        let la = f64::to_radians(pl);
        for k in 0..=72 {
            let lo = (k as f64 * 5.0).to_radians();
            let (p, front) = f.project(lonlat_to_vec(lo, la));
            if front {
                pts.push(at(p));
            } else {
                strip(&mut pts, grid);
            }
        }
        strip(&mut pts, grid);
    }

    painter.circle_stroke(c, r, Stroke::new(1.5_f32, Color32::from_white_alpha(200)));
    // north mark on the rim
    let nd = egui::vec2(f.ny.0 as f32, f.ny.1 as f32);
    painter.circle_filled(c + nd * (r + 5.0), 3.0, Color32::from_rgb(230, 60, 50));
    pin(painter, c);
}

/// A place marker whose tip is at `tip`.
fn pin(painter: &egui::Painter, tip: Pos2) {
    let head_r = 5.5_f32;
    let head = tip + egui::vec2(0.0, -14.0);
    let red = Color32::from_rgb(222, 48, 40);
    // a white outline first (slightly bigger shapes), then the red pin on top
    for (grow, col) in [(1.6_f32, Color32::WHITE), (0.0, red)] {
        let hr = head_r + grow;
        painter.circle_filled(head, hr, col);
        // the point: a triangle from the sides of the head down to the tip
        let ang = 0.9_f32;
        let l = head + egui::vec2(-hr * ang.cos(), hr * ang.sin());
        let rr = head + egui::vec2(hr * ang.cos(), hr * ang.sin());
        let t = tip + egui::vec2(0.0, grow * 0.9);
        painter.add(egui::Shape::convex_polygon(vec![l, rr, t], col, Stroke::NONE));
    }
    painter.circle_filled(head, 2.1, Color32::WHITE);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baked_land_bitmap_knows_land_from_sea() {
        let l = Land::load();
        assert!(!l.is_empty());
        let at = |lon: f64, lat: f64| l.at(lon.to_radians(), lat.to_radians());
        assert!(at(-98.0, 38.0) > 0.9, "Kansas is land");
        assert!(at(10.0, 25.0) > 0.9, "the Sahara is land");
        assert!(at(139.7, 35.7) > 0.5, "Tokyo is land");
        assert!(at(-150.0, 0.0) < 0.1, "the Pacific is sea");
        assert!(at(-30.0, 30.0) < 0.1, "the Atlantic is sea");
        assert!(at(179.9, 0.0) < 0.1 && at(-179.9, 0.0) < 0.1, "wraps cleanly at the date line");
    }

    #[test]
    fn the_rendered_globe_has_land_sea_and_a_clear_outside() {
        let l = Land::load();
        // centred on Africa
        let img = l.render(96, 0.3, 0.2, 0.0);
        assert_eq!(img.size, [96, 96]);
        assert_eq!(img[(0, 0)].a(), 0, "corners are outside the disc");
        let centre = img[(48, 48)];
        assert!(centre.a() == 255 && centre.g() > centre.b(), "Africa is green, not blue: {centre:?}");
        // the Atlantic to the west of it is blue
        let west = img[(10, 48)];
        assert!(west.b() > west.g() && west.b() > west.r(), "sea is blue: {west:?}");
        // turning the map by 180 degrees swaps west and east
        let flipped = l.render(96, 0.3, 0.2, std::f64::consts::PI);
        assert!(flipped[(86, 48)].b() > flipped[(86, 48)].g(), "sea is now on the right");
    }

    #[test]
    fn the_pixel_to_planet_mapping_matches_the_projection() {
        // a point projected onto the disc must be found again when tracing that pixel back
        let (lon, lat, heading) = (0.7, 0.4, 0.9);
        let f = Frame::new(lon, lat, heading);
        let target = lonlat_to_vec(0.9, 0.5);
        let ([sx, sy], front) = f.project(target);
        assert!(front);
        let (s, c) = heading.sin_cos();
        let z = (1.0 - (sx * sx + sy * sy) as f64).sqrt();
        let (x, y) = (c * sx as f64 - s * sy as f64, -s * sx as f64 - c * sy as f64);
        let back = [
            x * f.east[0] + y * f.north[0] + z * f.n[0],
            x * f.east[1] + y * f.north[1] + z * f.n[1],
            x * f.east[2] + y * f.north[2] + z * f.n[2],
        ];
        assert!(dot(back, target) > 0.999999, "{back:?} vs {target:?}");
    }

    #[test]
    fn the_centre_faces_the_viewer_and_north_is_up() {
        let f = Frame::new(0.3, 0.5, 0.0);
        let (p, front) = f.project(lonlat_to_vec(0.3, 0.5));
        assert!(front && p[0].abs() < 1e-6 && p[1].abs() < 1e-6);
        let (north, _) = f.project(lonlat_to_vec(0.3, 0.8));
        assert!(north[1] < -0.1, "north is up on screen: {north:?}");
        let (east, _) = f.project(lonlat_to_vec(0.6, 0.5));
        assert!(east[0] > 0.1, "east is right: {east:?}");
        // the far side is hidden
        let (_, front) = f.project(lonlat_to_vec(0.3 + 3.0, -0.5));
        assert!(!front);
    }

    #[test]
    fn turning_the_map_turns_the_globe() {
        // heading 90 degrees: north points to the left of the screen
        let f = Frame::new(0.0, 0.0, std::f64::consts::FRAC_PI_2);
        let (north, _) = f.project(lonlat_to_vec(0.0, 0.3));
        assert!(north[0] < -0.1 && north[1].abs() < 0.05, "{north:?}");
    }
}
