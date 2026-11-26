//! The small globe in the corner: it turns to face the place the main view is looking at, keeps the
//! same orientation as the map, and flashes a dot where you are. The coastlines are Natural Earth
//! (public domain), baked into `assets/coast.bin` by `tools/make_coast.py`.

use eframe::egui::{self, Color32, Pos2, Stroke};

use crate::camera::{cross, dot, lonlat_to_vec, normalize, V3};

/// Coastline polylines as unit vectors.
pub struct Coast {
    lines: Vec<Vec<V3>>,
}

impl Coast {
    pub fn load() -> Coast {
        Coast::parse(include_bytes!("../assets/coast.bin"))
    }

    fn parse(b: &[u8]) -> Coast {
        let mut lines = Vec::new();
        let mut at = 0usize;
        while at + 2 <= b.len() {
            let n = u16::from_le_bytes([b[at], b[at + 1]]) as usize;
            at += 2;
            if at + n * 4 > b.len() {
                break;
            }
            let mut pts = Vec::with_capacity(n);
            for k in 0..n {
                let o = at + k * 4;
                let lon = i16::from_le_bytes([b[o], b[o + 1]]) as f64 / 100.0;
                let lat = i16::from_le_bytes([b[o + 2], b[o + 3]]) as f64 / 100.0;
                pts.push(lonlat_to_vec(lon.to_radians(), lat.to_radians()));
            }
            at += n * 4;
            lines.push(pts);
        }
        Coast { lines }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
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

/// Draws the globe centred at `c` with radius `r` points. `blink` switches the position dot on/off.
pub fn draw(painter: &egui::Painter, c: Pos2, r: f32, coast: &Coast, lon: f64, lat: f64, heading: f64, blink: bool) {
    let f = Frame::new(lon, lat, heading);
    painter.circle_filled(c, r, Color32::from_rgb(18, 40, 78));
    let at = |p: [f32; 2]| Pos2::new(c.x + p[0] * r, c.y + p[1] * r);

    // graticule every 30 degrees
    let grid = Stroke::new(0.6_f32, Color32::from_white_alpha(38));
    let mut strip = |pts: &mut Vec<Pos2>, stroke: Stroke| {
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

    // coastlines
    let coast_stroke = Stroke::new(1.3_f32, Color32::from_rgb(170, 210, 150));
    for line in &coast.lines {
        for q in line {
            let (p, front) = f.project(*q);
            if front {
                pts.push(at(p));
            } else {
                strip(&mut pts, coast_stroke);
            }
        }
        strip(&mut pts, coast_stroke);
    }

    painter.circle_stroke(c, r, Stroke::new(1.5_f32, Color32::from_white_alpha(200)));
    // north mark on the rim
    let (np, _) = f.project([0.0, 1.0, 0.0]);
    let nd = egui::vec2(f.ny.0 as f32, f.ny.1 as f32);
    let _ = np;
    painter.circle_filled(c + nd * (r + 5.0), 3.0, Color32::from_rgb(230, 60, 50));
    // you are here
    if blink {
        painter.circle_filled(c, 4.5, Color32::from_rgb(255, 70, 60));
        painter.circle_stroke(c, 6.5, Stroke::new(1.5_f32, Color32::WHITE));
    } else {
        painter.circle_filled(c, 2.0, Color32::from_rgb(255, 120, 110));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baked_coastlines_load() {
        let c = Coast::load();
        assert!(c.lines.len() > 50);
        assert!(c.lines.iter().all(|l| l.len() >= 2));
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
