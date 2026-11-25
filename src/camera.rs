//! Globe camera. Std-only math, f64 throughout.
//!
//! The planet is a unit sphere. The camera floats at `1 + alt` planet radii
//! above a lon/lat point, looking straight down at the planet's centre with
//! north at the top of the screen. Zooming in lowers `alt`; at city scale the
//! visible patch of sphere is so small that it simply looks flat.

use std::f64::consts::PI;

pub type V3 = [f64; 3];

/// Vertical field of view (45 degrees).
pub const FOV_Y: f64 = std::f64::consts::FRAC_PI_4;
/// Lowest allowed altitude, in planet radii (~5 km). Limited by f32 precision on the GPU.
pub const MIN_ALT: f64 = 0.0008;
pub const MAX_ALT: f64 = 6.0;
/// 85 degrees in radians (just inside the Web Mercator limit).
pub const MAX_LAT: f64 = 1.4835298641951802;

pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
pub fn normalize(a: V3) -> V3 {
    let n = dot(a, a).sqrt();
    if n == 0.0 {
        a
    } else {
        scale(a, 1.0 / n)
    }
}

/// x = east at lon 0 + 90, y = north, z = towards lon 0 on the equator.
pub fn lonlat_to_vec(lon: f64, lat: f64) -> V3 {
    let cl = lat.cos();
    [cl * lon.sin(), lat.sin(), cl * lon.cos()]
}

pub fn vec_to_lonlat(p: V3) -> (f64, f64) {
    (p[0].atan2(p[2]), p[1].clamp(-1.0, 1.0).asin())
}

/// Normalized Web Mercator (0..1, y down) to unit-sphere position.
pub fn merc_to_vec(mx: f64, my: f64) -> V3 {
    let lon = (mx - 0.5) * 2.0 * PI;
    let lat = (PI * (1.0 - 2.0 * my)).sinh().atan();
    lonlat_to_vec(lon, lat)
}

pub fn lonlat_to_merc(lon: f64, lat: f64) -> (f64, f64) {
    let mx = lon / (2.0 * PI) + 0.5;
    let my = 0.5 - (PI / 4.0 + lat / 2.0).tan().ln() / (2.0 * PI);
    (mx, my)
}

pub fn wrap_pi(a: f64) -> f64 {
    (a + PI).rem_euclid(2.0 * PI) - PI
}

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub lon: f64,
    pub lat: f64,
    /// Height above the surface in planet radii.
    pub alt: f64,
}

impl Camera {
    pub fn from_degrees(lon_deg: f64, lat_deg: f64, alt: f64) -> Self {
        let mut c = Camera { lon: lon_deg.to_radians(), lat: lat_deg.to_radians(), alt };
        c.normalize();
        c
    }

    pub fn normalize(&mut self) {
        self.lon = wrap_pi(self.lon);
        self.lat = self.lat.clamp(-MAX_LAT, MAX_LAT);
        self.alt = self.alt.clamp(MIN_ALT, MAX_ALT);
    }

    /// Unit vector from the planet centre towards the camera.
    pub fn dir(&self) -> V3 {
        lonlat_to_vec(self.lon, self.lat)
    }

    pub fn dist(&self) -> f64 {
        1.0 + self.alt
    }

    pub fn pos(&self) -> V3 {
        scale(self.dir(), self.dist())
    }

    /// (right, up, forward) in world space.
    pub fn basis(&self) -> (V3, V3, V3) {
        let f = scale(self.dir(), -1.0);
        let north = [0.0, 1.0, 0.0];
        let right = normalize(cross(f, north));
        let up = cross(right, f);
        (right, up, f)
    }

    /// Row-major view-projection matrix (right-handed, depth 0..1).
    pub fn view_proj(&self, aspect: f64) -> [[f64; 4]; 4] {
        let c = self.pos();
        let (r, u, f) = self.basis();
        let v = [
            [r[0], r[1], r[2], -dot(r, c)],
            [u[0], u[1], u[2], -dot(u, c)],
            [-f[0], -f[1], -f[2], dot(f, c)],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let near = (self.alt * 0.05).max(1e-6);
        let far = self.dist() + 1.0;
        let t = 1.0 / (FOV_Y * 0.5).tan();
        let p = [
            [t / aspect, 0.0, 0.0, 0.0],
            [0.0, t, 0.0, 0.0],
            [0.0, 0.0, far / (near - far), near * far / (near - far)],
            [0.0, 0.0, -1.0, 0.0],
        ];
        let mut m = [[0.0; 4]; 4];
        for (i, row) in m.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = (0..4).map(|k| p[i][k] * v[k][j]).sum();
            }
        }
        m
    }

    /// Column-major f32 matrix, ready for a WGSL `mat4x4<f32>`.
    pub fn view_proj_cols(&self, aspect: f64) -> [f32; 16] {
        let m = self.view_proj(aspect);
        let mut out = [0f32; 16];
        for col in 0..4 {
            for row in 0..4 {
                out[col * 4 + row] = m[row][col] as f32;
            }
        }
        out
    }

    /// Projects a world-space point to screen pixels (origin top-left).
    pub fn project(&self, p: V3, w: f64, h: f64) -> Option<[f64; 2]> {
        let (r, u, f) = self.basis();
        let rel = sub(p, self.pos());
        let z = dot(rel, f);
        if z <= 1e-9 {
            return None;
        }
        let t = (FOV_Y * 0.5).tan();
        let aspect = w / h;
        let nx = dot(rel, r) / (z * t * aspect);
        let ny = dot(rel, u) / (z * t);
        Some([(nx * 0.5 + 0.5) * w, (0.5 - ny * 0.5) * h])
    }

    /// The (lon, lat) of the planet surface under a screen pixel, if any.
    pub fn pick(&self, sx: f64, sy: f64, w: f64, h: f64) -> Option<(f64, f64)> {
        let (r, u, f) = self.basis();
        let t = (FOV_Y * 0.5).tan();
        let nx = (sx / w) * 2.0 - 1.0;
        let ny = 1.0 - (sy / h) * 2.0;
        let dir = normalize(add(add(scale(r, nx * t * (w / h)), scale(u, ny * t)), f));
        let c = self.pos();
        let b = dot(c, dir);
        let disc = b * b - (dot(c, c) - 1.0);
        if disc < 0.0 {
            return None;
        }
        let t0 = -b - disc.sqrt();
        if t0 < 0.0 {
            return None;
        }
        Some(vec_to_lonlat(add(c, scale(dir, t0))))
    }

    /// Fractional web-map zoom level for the current altitude, assuming 512-pixel tiles.
    pub fn zoom_f(&self, h: f64) -> f64 {
        let cos_lat = self.lat.cos().max(0.05);
        (2.0 * PI * cos_lat * h / (512.0 * 2.0 * self.alt * (FOV_Y * 0.5).tan())).log2()
    }

    /// Inverse of `zoom_f`: altitude that produces zoom `zf` at latitude `lat`.
    pub fn alt_for_zoom(zf: f64, lat: f64, h: f64) -> f64 {
        let cos_lat = lat.cos().max(0.05);
        2.0 * PI * cos_lat * h / (512.0 * 2.0 * (FOV_Y * 0.5).tan() * 2f64.powf(zf))
    }

    /// Drag by `dx`,`dy` screen pixels (content follows the cursor).
    pub fn pan(&mut self, dx: f64, dy: f64, h: f64) {
        let rad_per_px = 2.0 * self.alt * (FOV_Y * 0.5).tan() / h;
        let cos_lat = self.lat.cos().max(0.05);
        self.lon -= dx * rad_per_px / cos_lat;
        self.lat += dy * rad_per_px;
        self.normalize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn centre_projects_to_screen_centre() {
        let cam = Camera::from_degrees(-122.4, 37.8, 0.01);
        let p = cam.dir();
        let s = cam.project(p, 1000.0, 600.0).unwrap();
        assert!(close(s[0], 500.0, 1e-6) && close(s[1], 300.0, 1e-6), "{s:?}");
    }

    #[test]
    fn pick_inverts_project() {
        let cam = Camera::from_degrees(10.0, 50.0, 0.3);
        let (lon, lat) = (12.0f64.to_radians(), 51.0f64.to_radians());
        let s = cam.project(lonlat_to_vec(lon, lat), 1200.0, 700.0).unwrap();
        let (plon, plat) = cam.pick(s[0], s[1], 1200.0, 700.0).unwrap();
        assert!(close(plon, lon, 1e-9) && close(plat, lat, 1e-9));
    }

    #[test]
    fn north_is_up_and_east_is_right() {
        let cam = Camera::from_degrees(0.0, 0.0, 0.5);
        let c = cam.project(lonlat_to_vec(0.0, 0.0), 800.0, 800.0).unwrap();
        let n = cam.project(lonlat_to_vec(0.0, 5f64.to_radians()), 800.0, 800.0).unwrap();
        let e = cam.project(lonlat_to_vec(5f64.to_radians(), 0.0), 800.0, 800.0).unwrap();
        assert!(n[1] < c[1], "north should be above centre");
        assert!(e[0] > c[0], "east should be right of centre");
    }

    #[test]
    fn matrix_agrees_with_project() {
        let cam = Camera::from_degrees(-100.0, 30.0, 0.5);
        let (w, h) = (1000.0, 800.0);
        let m = cam.view_proj(w / h);
        let p = lonlat_to_vec(-95.0f64.to_radians(), 33.0f64.to_radians());
        let clip: Vec<f64> = (0..4).map(|r| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3]).collect();
        let sx = (clip[0] / clip[3] * 0.5 + 0.5) * w;
        let sy = (0.5 - clip[1] / clip[3] * 0.5) * h;
        let s = cam.project(p, w, h).unwrap();
        assert!(close(sx, s[0], 1e-6) && close(sy, s[1], 1e-6), "{sx},{sy} vs {s:?}");
        let z = clip[2] / clip[3];
        assert!((0.0..=1.0).contains(&z), "depth {z}");
    }

    #[test]
    fn mercator_round_trip() {
        let (lon, lat) = (-122.4f64.to_radians(), 37.7f64.to_radians());
        let (mx, my) = lonlat_to_merc(lon, lat);
        let p = merc_to_vec(mx, my);
        let (l2, a2) = vec_to_lonlat(p);
        assert!(close(l2, lon, 1e-9) && close(a2, lat, 1e-9));
    }

    #[test]
    fn zoom_helpers_are_inverse() {
        let lat = 37.8f64.to_radians();
        let alt = Camera::alt_for_zoom(12.5, lat, 900.0);
        let cam = Camera { lon: 0.0, lat, alt };
        assert!(close(cam.zoom_f(900.0), 12.5, 1e-9));
        // Globe view should land near zoom 2-3 for a normal window.
        let globe = Camera::from_degrees(0.0, 0.0, 2.2);
        let z = globe.zoom_f(800.0);
        assert!((1.0..4.0).contains(&z), "{z}");
    }

    #[test]
    fn pan_moves_content_with_cursor() {
        let mut cam = Camera::from_degrees(0.0, 0.0, 0.01);
        cam.pan(100.0, 0.0, 800.0);
        assert!(cam.lon < 0.0, "dragging right moves the camera west");
        let mut cam = Camera::from_degrees(0.0, 0.0, 0.01);
        cam.pan(0.0, 100.0, 800.0);
        assert!(cam.lat > 0.0, "dragging down moves the camera north");
    }
}
