//! The simulated flight: a great-circle route (San Francisco to Tokyo Haneda), an altitude
//! profile, and the maths that places the 3D plane model in the camera's view.
//! Std-only and unit-tested.

use crate::camera::{add, cross, dot, lonlat_to_vec, mul4, normalize, scale, sub, vec_to_lonlat, Camera, V3};

pub const EARTH_RADIUS_M: f64 = 6_371_000.0;
pub const CRUISE_ALT_M: f64 = 10_668.0; // 35,000 ft
pub const CRUISE_SPEED_MS: f64 = 210.0; // ~755 km/h average ground speed, about 11 h SFO-HND
/// Boeing 737 length (m): only used to size the camera framing.
pub const PLANE_LENGTH_M: f64 = 39.5;
/// A sphere (m, around the model origin) that contains the whole model.
pub const PLANE_RADIUS_M: f64 = 21.0;

pub const SFO: (f64, f64) = (-122.3790, 37.6213);
pub const HND: (f64, f64) = (139.7798, 35.5494);

#[derive(Clone, Copy, Debug)]
pub struct Route {
    a: V3,
    b: V3,
    omega: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct FlightState {
    /// Unit vector under the plane.
    pub ground: V3,
    pub alt_m: f64,
    /// Radians, positive nose-up.
    pub pitch: f64,
    /// Horizontal forward direction (unit tangent along the route).
    pub forward: V3,
}

impl Route {
    pub fn new(from_lonlat_deg: (f64, f64), to_lonlat_deg: (f64, f64)) -> Self {
        let a = lonlat_to_vec(from_lonlat_deg.0.to_radians(), from_lonlat_deg.1.to_radians());
        let b = lonlat_to_vec(to_lonlat_deg.0.to_radians(), to_lonlat_deg.1.to_radians());
        let omega = dot(a, b).clamp(-1.0, 1.0).acos();
        Route { a, b, omega }
    }

    pub fn sfo_to_hnd() -> Self {
        Route::new(SFO, HND)
    }

    pub fn length_m(&self) -> f64 {
        self.omega * EARTH_RADIUS_M
    }

    /// Real-time duration at cruise speed, in seconds.
    pub fn duration_s(&self) -> f64 {
        self.length_m() / CRUISE_SPEED_MS
    }

    /// Point on the great circle, `t` in 0..=1 (spherical interpolation).
    pub fn point(&self, t: f64) -> V3 {
        let s = self.omega.sin();
        if s.abs() < 1e-12 {
            return self.a;
        }
        let w0 = ((1.0 - t) * self.omega).sin() / s;
        let w1 = (t * self.omega).sin() / s;
        add(scale(self.a, w0), scale(self.b, w1))
    }

    fn tangent(&self, t: f64) -> V3 {
        let e = 1e-5;
        let (t0, t1) = ((t - e).max(0.0), (t + e).min(1.0));
        let p = self.point(t);
        let d = sub(self.point(t1), self.point(t0));
        // remove any radial part, keep it horizontal
        normalize(sub(d, scale(p, dot(d, p))))
    }

    pub fn altitude_m(&self, t: f64) -> f64 {
        let smooth = |x: f64| {
            let x = x.clamp(0.0, 1.0);
            x * x * (3.0 - 2.0 * x)
        };
        let up = smooth(t / 0.04);
        let down = smooth((1.0 - t) / 0.05);
        CRUISE_ALT_M * up.min(down)
    }

    pub fn state(&self, t: f64) -> FlightState {
        let t = t.clamp(0.0, 1.0);
        let e = 1e-4;
        let (t0, t1) = ((t - e).max(0.0), (t + e).min(1.0));
        let dh = self.altitude_m(t1) - self.altitude_m(t0);
        let ds = (t1 - t0) * self.length_m();
        // Real flight paths are shallow; exaggerate a little so the nose visibly tips up/down.
        let pitch = ((dh / ds).atan() * 4.0).clamp(-0.2, 0.2);
        FlightState { ground: self.point(t), alt_m: self.altitude_m(t), pitch, forward: self.tangent(t) }
    }

    /// Points along the whole route for drawing (unit vectors on the ground).
    pub fn polyline(&self, n: usize) -> Vec<V3> {
        (0..=n).map(|i| self.point(i as f64 / n as f64)).collect()
    }

    pub fn lonlat_deg(&self, t: f64) -> (f64, f64) {
        let (lo, la) = vec_to_lonlat(self.point(t));
        (lo.to_degrees(), la.to_degrees())
    }
}

/// Plane position (planet radii from the centre) and body axes in world space.
pub fn plane_frame(st: &FlightState) -> (V3, V3, V3, V3) {
    let up_h = st.ground;
    let right = normalize(cross(st.forward, up_h));
    let (sp, cp) = (st.pitch.sin(), st.pitch.cos());
    let fwd = add(scale(st.forward, cp), scale(up_h, sp));
    let up = sub(scale(up_h, cp), scale(st.forward, sp));
    let pos = scale(st.ground, 1.0 + st.alt_m / EARTH_RADIUS_M);
    (pos, right, fwd, up)
}

/// Matrices for drawing the model at `size_mul` times its real size:
/// (model-view-projection, normal matrix mapping model normals to view space), column-major,
/// or `None` when the plane cannot be seen (behind the camera or hidden by the planet).
/// Everything is composed in f64 so there is no jitter however close the camera gets.
pub fn plane_matrices(cam: &Camera, aspect: f64, st: &FlightState, size_mul: f64) -> Option<([f32; 16], [f32; 16])> {
    let (pos, right, fwd, up) = plane_frame(st);
    let s = size_mul / EARTH_RADIUS_M; // planet radii per metre
    let radius = PLANE_RADIUS_M * s;
    let c = cam.pos();
    let (cr, cu, cf) = cam.basis();

    let rel = sub(pos, c);
    let dist = dot(rel, rel).sqrt();
    if dot(rel, cf) < -radius {
        return None; // behind the camera
    }
    // Hidden by the planet? (closest approach of the sight line to the centre is inside the sphere)
    let t = (-dot(c, rel) / dot(rel, rel)).clamp(0.0, 1.0);
    let closest = add(c, scale(rel, t));
    if dot(closest, closest).sqrt() < 1.0 - 1e-9 {
        return None;
    }

    // Per-object clip range keeps depth precision high even for a tiny model far away.
    let near = (dist - radius * 1.5).max(dist * 0.01).max(1e-12);
    let far = dist + radius * 1.5;
    let proj = cam.proj_nf(aspect, near, far);
    let model = [
        [right[0] * s, fwd[0] * s, up[0] * s, pos[0]],
        [right[1] * s, fwd[1] * s, up[1] * s, pos[1]],
        [right[2] * s, fwd[2] * s, up[2] * s, pos[2]],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let mvp = mul4(&mul4(&proj, &cam.view()), &model);

    // Normal matrix = view rotation * model rotation (no scale).
    let view_rows = [cr, cu, [-cf[0], -cf[1], -cf[2]]];
    let cols = [right, fwd, up];
    let mut nrm = [[0.0f64; 4]; 4];
    for (i, row) in view_rows.iter().enumerate() {
        for (j, col) in cols.iter().enumerate() {
            nrm[i][j] = dot(*row, *col);
        }
    }
    nrm[3][3] = 1.0;

    let to_cols = |m: &[[f64; 4]; 4]| {
        let mut out = [0f32; 16];
        for col in 0..4 {
            for row in 0..4 {
                out[col * 4 + row] = m[row][col] as f32;
            }
        }
        out
    };
    Some((to_cols(&mvp), to_cols(&nrm)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_is_a_great_circle_of_plausible_length() {
        let r = Route::sfo_to_hnd();
        let km = r.length_m() / 1000.0;
        assert!((8000.0..8500.0).contains(&km), "{km}");
        let (lo, la) = r.lonlat_deg(0.0);
        assert!((lo - SFO.0).abs() < 1e-6 && (la - SFO.1).abs() < 1e-6);
        let (lo, la) = r.lonlat_deg(1.0);
        assert!((lo - HND.0).abs() < 1e-6 && (la - HND.1).abs() < 1e-6);
        // the great circle bulges north of both airports
        let (_, mid_lat) = r.lonlat_deg(0.5);
        assert!(mid_lat > 45.0, "{mid_lat}");
        assert!((10.5..11.5).contains(&(r.duration_s() / 3600.0)));
    }

    #[test]
    fn altitude_profile_climbs_cruises_descends() {
        let r = Route::sfo_to_hnd();
        assert_eq!(r.altitude_m(0.0), 0.0);
        assert!((r.altitude_m(0.5) - CRUISE_ALT_M).abs() < 1e-6);
        assert!(r.altitude_m(0.02) > 1000.0 && r.altitude_m(0.02) < CRUISE_ALT_M);
        assert!(r.altitude_m(1.0) < 1e-6);
        assert!(r.state(0.02).pitch > 0.05);
        assert!(r.state(0.98).pitch < 0.0);
        assert!(r.state(0.5).pitch.abs() < 1e-3);
    }

    #[test]
    fn nose_points_up_the_screen_when_flying_north() {
        let r = Route::new((0.0, 0.0), (0.0, 10.0));
        let st = r.state(0.5);
        let (lon, lat) = r.lonlat_deg(0.5);
        let cam = Camera { lon: lon.to_radians(), lat: lat.to_radians(), alt: 0.05, pitch: 0.0 };
        let (w, h) = (1000.0, 600.0);
        let size = 100.0;
        let (mvp, _n) = plane_matrices(&cam, w / h, &st, size).expect("visible");
        let apply = |p: [f32; 3]| {
            let m = &mvp;
            let x = m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12];
            let y = m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13];
            let wv = m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15];
            (x / wv, y / wv)
        };
        let (cx, cy) = apply([0.0, 0.0, 0.0]);
        let (nx, ny) = apply([0.0, 19.0, 0.0]); // nose
        let (rx, _ry) = apply([17.0, 0.0, 0.0]); // right wing tip
        assert!(cx.abs() < 0.05 && cy.abs() < 0.2, "{cx} {cy}");
        assert!(ny > cy + 1e-4 && (nx - cx).abs() < 1e-3, "nose {nx},{ny} centre {cx},{cy}");
        assert!(rx > cx, "starboard wing should be on the right when heading north");
    }

    #[test]
    fn hidden_behind_planet_or_camera() {
        let r = Route::new((0.0, 0.0), (0.0, 10.0));
        let st = r.state(0.5);
        // camera on the far side of the planet
        let far = Camera { lon: std::f64::consts::PI, lat: 0.0, alt: 0.5, pitch: 0.0 };
        assert!(plane_matrices(&far, 1.5, &st, 100.0).is_none());
        // camera lower than the plane, looking down: plane is behind it
        let low = Camera { lon: 0.0, lat: 5.0f64.to_radians(), alt: 0.0002, pitch: 0.0 };
        assert!(plane_matrices(&low, 1.5, &st, 1.0).is_none());
    }
}
