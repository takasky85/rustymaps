//! Globe camera. Std-only math, f64 throughout.
//!
//! The planet is a unit sphere. The camera orbits a lon/lat target point at distance
//! `alt` (planet radii), looking at it with north at the top of the screen. With
//! `pitch == 0` it hovers straight above the target; tilting swings it south, like
//! a map app's 3D view. Zooming in lowers `alt`; at city scale the visible patch of
//! sphere is so small that it simply looks flat.

use std::f64::consts::PI;

pub type V3 = [f64; 3];

/// Vertical field of view (45 degrees).
pub const FOV_Y: f64 = std::f64::consts::FRAC_PI_4;
/// Lowest allowed altitude, in planet radii (~130 m above the ground).
pub const MIN_ALT: f64 = 0.00002;
/// Largest tilt (about 72 degrees from straight down).
pub const MAX_PITCH: f64 = 1.25;
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

pub fn mul4(a: &[[f64; 4]; 4], b: &[[f64; 4]; 4]) -> [[f64; 4]; 4] {
    let mut m = [[0.0; 4]; 4];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..4).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}

/// Column-major f32 matrix, ready for a WGSL `mat4x4<f32>`.
pub fn cols_f32(m: &[[f64; 4]; 4]) -> [f32; 16] {
    let mut out = [0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = m[row][col] as f32;
        }
    }
    out
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
    /// Distance from the target point to the camera, in planet radii.
    pub alt: f64,
    /// Tilt away from straight-down, radians (0..=MAX_PITCH).
    pub pitch: f64,
    /// Compass bearing of the screen's "up" direction on the ground, radians (0 = north up).
    pub heading: f64,
    /// Where the target sits on screen, as an offset from the centre in normalised device units
    /// (x right, y up; the screen spans -1..1). Lets you drag the whole globe around the window.
    pub shift: [f64; 2],
}

/// How far the view may be shifted off-centre (normalised device units).
pub const MAX_SHIFT: f64 = 1.3;

impl Camera {
    pub fn from_degrees(lon_deg: f64, lat_deg: f64, alt: f64) -> Self {
        let mut c = Camera { lon: lon_deg.to_radians(), lat: lat_deg.to_radians(), alt, pitch: 0.0, heading: 0.0, shift: [0.0, 0.0] };
        c.normalize();
        c
    }

    pub fn normalize(&mut self) {
        self.lon = wrap_pi(self.lon);
        self.lat = self.lat.clamp(-MAX_LAT, MAX_LAT);
        self.alt = self.alt.clamp(MIN_ALT, MAX_ALT);
        self.pitch = self.pitch.clamp(0.0, MAX_PITCH);
        self.heading = wrap_pi(self.heading);
        self.shift = [self.shift[0].clamp(-MAX_SHIFT, MAX_SHIFT), self.shift[1].clamp(-MAX_SHIFT, MAX_SHIFT)];
    }

    /// Ground direction the camera looks along, in the target's (east, north) frame.
    fn look_en(&self) -> (f64, f64) {
        (self.heading.sin(), self.heading.cos())
    }

    /// Straight-down, north-up view.
    pub fn reset_orientation(&mut self) {
        self.pitch = 0.0;
        self.heading = 0.0;
        self.shift = [0.0, 0.0];
    }

    /// Move the target on screen by `dx`,`dy` pixels (the globe follows the cursor).
    pub fn shift_by(&mut self, dx: f64, dy: f64, w: f64, h: f64) {
        self.shift[0] += dx / w * 2.0;
        self.shift[1] -= dy / h * 2.0;
        self.normalize();
    }

    /// Unit vector from the planet centre towards the target point.
    pub fn dir(&self) -> V3 {
        lonlat_to_vec(self.lon, self.lat)
    }

    /// Local frame at the target: (east, north, up).
    pub fn frame(&self) -> (V3, V3, V3) {
        let u = self.dir();
        let e = normalize(cross([0.0, 1.0, 0.0], u));
        let n = cross(u, e);
        (e, n, u)
    }

    /// Camera position in world space.
    pub fn pos(&self) -> V3 {
        let (_, n, u) = self.frame();
        let (e, _, _) = self.frame();
        let up = 1.0 + self.alt * self.pitch.cos();
        let (he, hn) = self.look_en();
        let back = self.alt * self.pitch.sin();
        add(scale(u, up), add(scale(e, -back * he), scale(n, -back * hn)))
    }

    /// Distance of the camera from the planet's centre.
    pub fn dist(&self) -> f64 {
        let p = self.pos();
        dot(p, p).sqrt()
    }

    /// (right, up, forward) in world space.
    pub fn basis(&self) -> (V3, V3, V3) {
        let (e, n, u) = self.frame();
        let (sp, cp) = (self.pitch.sin(), self.pitch.cos());
        let (he, hn) = self.look_en();
        let h = add(scale(e, he), scale(n, hn));
        let f = add(scale(u, -cp), scale(h, sp));
        let r = add(scale(e, hn), scale(n, -he));
        let up = cross(r, f);
        (r, up, f)
    }

    /// Row-major perspective projection (right-handed, depth 0..1).
    pub fn proj(&self, aspect: f64) -> [[f64; 4]; 4] {
        self.proj_nf(aspect, (self.alt * 0.05).max(1e-7), self.dist() + 1.0)
    }

    /// Perspective projection with explicit clip distances.
    pub fn proj_nf(&self, aspect: f64, near: f64, far: f64) -> [[f64; 4]; 4] {
        let t = 1.0 / (FOV_Y * 0.5).tan();
        // The screen shift is added in normalised device units: x' = x + shift * w, and w = -z.
        [
            [t / aspect, 0.0, -self.shift[0], 0.0],
            [0.0, t, -self.shift[1], 0.0],
            [0.0, 0.0, far / (near - far), near * far / (near - far)],
            [0.0, 0.0, -1.0, 0.0],
        ]
    }

    /// Row-major world -> view matrix.
    pub fn view(&self) -> [[f64; 4]; 4] {
        let c = self.pos();
        let (r, u, f) = self.basis();
        [
            [r[0], r[1], r[2], -dot(r, c)],
            [u[0], u[1], u[2], -dot(u, c)],
            [-f[0], -f[1], -f[2], dot(f, c)],
            [0.0, 0.0, 0.0, 1.0],
        ]
    }

    /// Row-major world -> clip matrix (right-handed, depth 0..1).
    pub fn view_proj(&self, aspect: f64) -> [[f64; 4]; 4] {
        mul4(&self.proj(aspect), &self.view())
    }

    /// Clip matrix for positions given *relative to the target point* in its local
    /// (east, north, radial-offset) frame, which is how the tile shader works, so that
    /// precision never degrades with zoom.
    pub fn rel_proj(&self, aspect: f64) -> [[f64; 4]; 4] {
        let (sp, cp) = (self.pitch.sin(), self.pitch.cos());
        let (sh, ch) = (self.heading.sin(), self.heading.cos());
        let v = [
            [ch, -sh, 0.0, 0.0],
            [cp * sh, cp * ch, sp, 0.0],
            [-sp * sh, -sp * ch, cp, -self.alt],
            [0.0, 0.0, 0.0, 1.0],
        ];
        mul4(&self.proj(aspect), &v)
    }

    pub fn view_proj_cols(&self, aspect: f64) -> [f32; 16] {
        cols_f32(&self.view_proj(aspect))
    }

    pub fn rel_proj_cols(&self, aspect: f64) -> [f32; 16] {
        cols_f32(&self.rel_proj(aspect))
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
        let nx = dot(rel, r) / (z * t * aspect) + self.shift[0];
        let ny = dot(rel, u) / (z * t) + self.shift[1];
        Some([(nx * 0.5 + 0.5) * w, (0.5 - ny * 0.5) * h])
    }

    /// The (lon, lat) of the planet surface under a screen pixel, if any.
    pub fn pick(&self, sx: f64, sy: f64, w: f64, h: f64) -> Option<(f64, f64)> {
        let (r, u, f) = self.basis();
        let t = (FOV_Y * 0.5).tan();
        let nx = (sx / w) * 2.0 - 1.0 - self.shift[0];
        let ny = 1.0 - (sy / h) * 2.0 - self.shift[1];
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
        let dyv = dy / self.pitch.cos().max(0.3);
        let (sh, ch) = (self.heading.sin(), self.heading.cos());
        let d_east = -dx * ch + dyv * sh;
        let d_north = dx * sh + dyv * ch;
        self.lon += d_east * rad_per_px / cos_lat;
        self.lat += d_north * rad_per_px;
        self.normalize();
    }

    /// Right- or middle-drag: vertical = tilt, horizontal = rotate around the target.
    pub fn orbit(&mut self, dx: f64, dy: f64) {
        self.pitch -= dy * 0.006;
        self.heading += dx * 0.006;
        self.normalize();
    }
}

/// Camera basis and constants precomputed once per frame, for projecting many points quickly.
#[derive(Clone, Copy)]
pub struct Projector {
    c: V3,
    r: V3,
    u: V3,
    f: V3,
    kx: f64,
    ky: f64,
    sx: f64,
    sy: f64,
    w: f64,
    h: f64,
}

impl Camera {
    pub fn projector(&self, w: f64, h: f64) -> Projector {
        let (r, u, f) = self.basis();
        let t = (FOV_Y * 0.5).tan();
        Projector { c: self.pos(), r, u, f, kx: t * (w / h), ky: t, sx: self.shift[0], sy: self.shift[1], w, h }
    }
}

impl Projector {
    /// True when `p` (a point on or above the unit sphere) is on the side of the planet the camera sees.
    pub fn visible(&self, p: V3) -> bool {
        dot(p, self.c) > 1.0 + 1e-9
    }

    /// Screen pixels (origin top-left) of a world point, or `None` when it is behind the camera.
    pub fn project(&self, p: V3) -> Option<[f64; 2]> {
        let rel = sub(p, self.c);
        let z = dot(rel, self.f);
        if z <= 1e-9 {
            return None;
        }
        let nx = dot(rel, self.r) / (z * self.kx) + self.sx;
        let ny = dot(rel, self.u) / (z * self.ky) + self.sy;
        Some([(nx * 0.5 + 0.5) * self.w, (0.5 - ny * 0.5) * self.h])
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
        let cam = Camera { lon: 0.0, lat, alt, pitch: 0.0, heading: 0.0, shift: [0.0, 0.0] };
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

    /// f64 mirror of the shader's cancellation-free camera-relative position:
    /// (east, north, radial offset) of the surface point at mercator-radian offset
    /// `dy` and longitude offset `dl` from the point under the camera.
    fn rel_formula(lat_c: f64, dl: f64, dy: f64) -> V3 {
        let (cc, sc) = (lat_c.cos(), lat_c.sin());
        let t = dy.tanh();
        let sg = 1.0 / dy.cosh();
        let d = 1.0 + sc * t;
        let sh = (0.5 * dl).sin();
        let q = t * t / (1.0 + sg) + sg * 2.0 * sh * sh;
        [cc * sg * dl.sin() / d, cc * (sc * q + t) / d, -cc * cc * q / d]
    }

    #[test]
    fn camera_relative_formula_matches_absolute() {
        for &(lon, lat) in &[(-2.136, 0.659), (0.3, -0.2), (3.0, 1.2), (0.0, 0.0)] {
            let cam = Camera { lon, lat, alt: 0.1, pitch: 0.0, heading: 0.0, shift: [0.0, 0.0] };
            let y_c = lat.sin().atanh();
            let (r, u, _f) = cam.basis();
            let d = cam.dir();
            for &(dl, dy) in &[(0.0, 0.0), (1e-3, -2e-3), (0.3, 0.2), (-1.0, 0.9), (2.5, -2.0), (3.1, 2.5)] {
                let p = lonlat_to_vec(lon + dl, (y_c + dy).sinh().atan());
                let rel = sub(p, d);
                let want = [dot(rel, r), dot(rel, u), dot(rel, d)];
                let got = rel_formula(lat, dl, dy);
                for i in 0..3 {
                    assert!((want[i] - got[i]).abs() < 1e-9, "{lon} {lat} {dl} {dy}: {want:?} vs {got:?}");
                }
            }
        }
    }

    #[test]
    fn relative_matrix_agrees_with_world_matrix_under_tilt() {
        for &(pitch, heading) in &[(0.0, 0.0), (0.6, 0.0), (1.1, 0.0), (0.9, 1.0), (0.5, -2.3)] {
            let cam = Camera { lon: -2.136, lat: 0.659, alt: 0.01, pitch, heading, shift: [0.0, 0.0] };
            let y_c = cam.lat.sin().atanh();
            let (aspect, vp, rp) = (1.6, cam.view_proj(1.6), cam.rel_proj(1.6));
            let _ = aspect;
            for &(dl, dy) in &[(0.0, 0.0), (0.002, -0.003), (-0.004, 0.001)] {
                let p = lonlat_to_vec(cam.lon + dl, (y_c + dy).sinh().atan());
                let rel = rel_formula(cam.lat, dl, dy);
                let apply = |m: &[[f64; 4]; 4], v: [f64; 3]| {
                    let o: Vec<f64> = (0..4).map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2] + m[i][3]).collect();
                    [o[0] / o[3], o[1] / o[3]]
                };
                let a = apply(&vp, p);
                let b = apply(&rp, rel);
                assert!((a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6, "pitch {pitch} heading {heading}: {a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn tilt_keeps_target_at_screen_centre() {
        let cam = Camera { lon: 0.4, lat: 0.5, alt: 0.05, pitch: 0.9, heading: 0.7, shift: [0.0, 0.0] };
        let s = cam.project(cam.dir(), 1000.0, 600.0).unwrap();
        assert!((s[0] - 500.0).abs() < 1e-6 && (s[1] - 300.0).abs() < 1e-6, "{s:?}");
        let (lon, lat) = cam.pick(500.0, 300.0, 1000.0, 600.0).unwrap();
        assert!((lon - 0.4).abs() < 1e-9 && (lat - 0.5).abs() < 1e-9);
    }

    #[test]
    fn projector_matches_camera_project() {
        let cam = Camera { lon: 0.3, lat: 0.6, alt: 0.02, pitch: 0.8, heading: -1.1, shift: [0.0, 0.0] };
        let pr = cam.projector(1000.0, 700.0);
        let p = lonlat_to_vec(0.301, 0.6005);
        let a = cam.project(p, 1000.0, 700.0).unwrap();
        let b = pr.project(p).unwrap();
        assert!(close(a[0], b[0], 1e-9) && close(a[1], b[1], 1e-9));
        assert!(pr.visible(p));
        assert!(!pr.visible(lonlat_to_vec(0.3 + PI, -0.6)));
    }

    #[test]
    fn heading_rotates_the_view() {
        // Facing east: east is up the screen, north is to the left.
        let cam = Camera { lon: 0.0, lat: 0.0, alt: 0.5, pitch: 0.0, heading: std::f64::consts::FRAC_PI_2, shift: [0.0, 0.0] };
        let c = cam.project(lonlat_to_vec(0.0, 0.0), 800.0, 800.0).unwrap();
        let e = cam.project(lonlat_to_vec(0.05, 0.0), 800.0, 800.0).unwrap();
        let n = cam.project(lonlat_to_vec(0.0, 0.05), 800.0, 800.0).unwrap();
        assert!(e[1] < c[1] - 1.0 && (e[0] - c[0]).abs() < 1.0, "east should be up: {e:?}");
        assert!(n[0] < c[0] - 1.0 && (n[1] - c[1]).abs() < 1.0, "north should be left: {n:?}");
    }

    #[test]
    fn rotated_pan_moves_content_with_cursor() {
        let (w, h) = (1000.0, 800.0);
        for &heading in &[0.0, 0.9, -2.0, 3.0] {
            let mut cam = Camera { lon: 0.2, lat: 0.4, alt: 0.01, pitch: 0.0, heading, shift: [0.0, 0.0] };
            let p = lonlat_to_vec(0.2005, 0.4003);
            let before = cam.project(p, w, h).unwrap();
            cam.pan(40.0, -25.0, h);
            let after = cam.project(p, w, h).unwrap();
            assert!((after[0] - before[0] - 40.0).abs() < 0.5 && (after[1] - before[1] + 25.0).abs() < 0.5, "{heading}: {before:?} {after:?}");
        }
    }

    #[test]
    fn shift_moves_the_target_on_screen_and_everything_stays_consistent() {
        let mut cam = Camera::from_degrees(-100.0, 30.0, 0.6);
        cam.pitch = 0.4;
        cam.heading = 0.7;
        cam.shift_by(-150.0, 90.0, 1000.0, 800.0);
        let (w, h) = (1000.0, 800.0);
        // the target is no longer at the centre, and moved the way the cursor did
        let s = cam.project(cam.dir(), w, h).unwrap();
        assert!(close(s[0], 500.0 - 150.0, 1e-6) && close(s[1], 400.0 + 90.0, 1e-6), "{s:?}");
        // pick inverts project
        let (lon, lat) = (-97.0f64.to_radians(), 31.0f64.to_radians());
        let sp = cam.project(lonlat_to_vec(lon, lat), w, h).unwrap();
        let (plon, plat) = cam.pick(sp[0], sp[1], w, h).unwrap();
        assert!(close(plon, lon, 1e-9) && close(plat, lat, 1e-9));
        // the matrices agree with project
        let m = cam.view_proj(w / h);
        let p = lonlat_to_vec(lon, lat);
        let clip: Vec<f64> = (0..4).map(|r| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3]).collect();
        assert!(close((clip[0] / clip[3] * 0.5 + 0.5) * w, sp[0], 1e-6) && close((0.5 - clip[1] / clip[3] * 0.5) * h, sp[1], 1e-6));
        // so does the projector
        let q = cam.projector(w, h).project(p).unwrap();
        assert!(close(q[0], sp[0], 1e-9) && close(q[1], sp[1], 1e-9));
        // reset puts it back in the middle
        cam.reset_orientation();
        let s = cam.project(cam.dir(), w, h).unwrap();
        assert!(close(s[0], 500.0, 1e-6) && close(s[1], 400.0, 1e-6));
    }
}
