//! The simulated flights: great-circle routes, altitude profiles, ping-pong playback (fly to the far
//! airport, wait, turn around, fly back, forever) and the maths that places each 3D aircraft in the
//! camera's view. Std-only and unit-tested.

use std::f64::consts::PI;

use crate::camera::{add, cross, dot, lonlat_to_vec, mul4, normalize, scale, sub, vec_to_lonlat, Camera, V3};

pub const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// A scheduled aircraft: which model to draw and where it flies.
pub struct AircraftDef {
    /// Model file stem: `data/models/<model>.bin`.
    pub model: &'static str,
    pub label: &'static str,
    pub from: (&'static str, f64, f64), // (airport code, lon, lat)
    pub to: (&'static str, f64, f64),
    /// Real length, for camera framing (the model file is scaled to match).
    pub length_m: f64,
    pub cruise_alt_m: f64,
    /// Average ground speed.
    pub speed_ms: f64,
    /// Route line colour.
    pub color: [u8; 3],
}

pub static AIRCRAFT: [AircraftDef; 3] = [
    AircraftDef {
        model: "b737",
        label: "Southwest 737",
        from: ("LAS", -115.1537, 36.0840),
        to: ("LBB", -101.8228, 33.6636),
        length_m: 39.5,
        cruise_alt_m: 10_058.0, // 33,000 ft
        speed_ms: 205.0,
        color: [255, 140, 0],
    },
    AircraftDef {
        model: "a380",
        label: "Lufthansa A380",
        from: ("SFO", -122.3790, 37.6213),
        to: ("FRA", 8.5622, 50.0379),
        length_m: 72.7,
        cruise_alt_m: 10_668.0, // 35,000 ft
        speed_ms: 245.0,
        color: [255, 205, 0],
    },
    AircraftDef {
        model: "b787",
        label: "ANA 787 Dreamliner",
        from: ("SFO", -122.3790, 37.6213),
        to: ("HND", 139.7798, 35.5494),
        length_m: 62.8,
        cruise_alt_m: 11_582.0, // 38,000 ft
        speed_ms: 210.0,
        color: [60, 170, 255],
    },
];

#[derive(Clone, Copy, Debug)]
pub struct Route {
    a: V3,
    b: V3,
    omega: f64,
    cruise_alt_m: f64,
    speed_ms: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct FlightState {
    /// Unit vector under the plane.
    pub ground: V3,
    /// Real height above the ground, metres.
    pub alt_m: f64,
    /// Radians, positive nose-up.
    pub pitch: f64,
    /// Horizontal forward direction (unit vector the nose points along).
    pub forward: V3,
}

fn smooth(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

impl Route {
    pub fn new(from_lonlat_deg: (f64, f64), to_lonlat_deg: (f64, f64), cruise_alt_m: f64, speed_ms: f64) -> Self {
        let a = lonlat_to_vec(from_lonlat_deg.0.to_radians(), from_lonlat_deg.1.to_radians());
        let b = lonlat_to_vec(to_lonlat_deg.0.to_radians(), to_lonlat_deg.1.to_radians());
        let omega = dot(a, b).clamp(-1.0, 1.0).acos();
        Route { a, b, omega, cruise_alt_m, speed_ms }
    }

    pub fn of(def: &AircraftDef) -> Self {
        Route::new((def.from.1, def.from.2), (def.to.1, def.to.2), def.cruise_alt_m, def.speed_ms)
    }

    pub fn length_m(&self) -> f64 {
        self.omega * EARTH_RADIUS_M
    }

    /// Real-time duration of one leg, in seconds.
    pub fn duration_s(&self) -> f64 {
        (self.length_m() / self.speed_ms).max(1.0)
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

    /// Height above the ground: climbs over (at most) 200 km, cruises, descends over (at most) 150 km.
    pub fn altitude_m(&self, t: f64) -> f64 {
        let l = self.length_m();
        let climb = (0.4 * l).min(200_000.0);
        let descent = (0.4 * l).min(150_000.0);
        if climb <= 0.0 || descent <= 0.0 {
            return 0.0;
        }
        let up = smooth(t * l / climb);
        let down = smooth((1.0 - t) * l / descent);
        self.cruise_alt_m * up.min(down)
    }

    /// State at route fraction `t`, flying towards the far end (`outbound`) or back, with the nose
    /// swung `yaw` radians away from that direction (used for the turn-around).
    pub fn state(&self, t: f64, outbound: bool, yaw: f64) -> FlightState {
        let t = t.clamp(0.0, 1.0);
        let e = 1e-4;
        let (t0, t1) = ((t - e).max(0.0), (t + e).min(1.0));
        let dh = self.altitude_m(t1) - self.altitude_m(t0);
        let ds = ((t1 - t0) * self.length_m()).max(1e-9);
        let sign = if outbound { 1.0 } else { -1.0 };
        // Real flight paths are shallow; exaggerate a little so the nose visibly tips up/down.
        let pitch = ((sign * dh / ds).atan() * 4.0).clamp(-0.2, 0.2);
        let ground = self.point(t);
        let base = scale(self.tangent(t), sign);
        let forward = if yaw == 0.0 {
            base
        } else {
            add(scale(base, yaw.cos()), scale(cross(ground, base), yaw.sin()))
        };
        FlightState { ground, alt_m: self.altitude_m(t), pitch, forward }
    }

    /// Points along the whole route for drawing (unit vectors on the ground).
    pub fn polyline(&self, n: usize) -> Vec<V3> {
        (0..=n).map(|i| self.point(i as f64 / n as f64)).collect()
    }

    #[allow(dead_code)]
    pub fn lonlat_deg(&self, t: f64) -> (f64, f64) {
        let (lo, la) = vec_to_lonlat(self.point(t));
        (lo.to_degrees(), la.to_degrees())
    }
}

/// How long (seconds of wall-clock time) an aircraft waits at the far end, turning around.
pub const TURN_PAUSE_S: f64 = 6.0;

/// Playback state of one aircraft: shuttles between the two airports until stopped.
#[derive(Clone, Copy, Debug)]
pub struct Sim {
    /// Position along the route, 0 = departure airport, 1 = destination.
    pub t: f64,
    /// True while flying from the departure airport towards the destination.
    pub outbound: bool,
    pub playing: bool,
    /// Seconds of turn-around pause left (0 = flying).
    pub pause: f64,
}

impl Sim {
    pub fn new(t: f64) -> Self {
        Sim { t, outbound: true, playing: false, pause: 0.0 }
    }

    /// Advances by `dt` wall-clock seconds at `speed` times real time.
    pub fn advance(&mut self, dt: f64, speed: f64, leg_duration_s: f64) {
        if !self.playing {
            return;
        }
        let mut dt = dt;
        if self.pause > 0.0 {
            self.pause -= dt;
            if self.pause > 0.0 {
                return;
            }
            dt = -self.pause;
            self.pause = 0.0;
            self.outbound = !self.outbound;
        }
        let step = dt * speed / leg_duration_s;
        if self.outbound {
            self.t += step;
            if self.t >= 1.0 {
                self.t = 1.0;
                self.pause = TURN_PAUSE_S;
            }
        } else {
            self.t -= step;
            if self.t <= 0.0 {
                self.t = 0.0;
                self.pause = TURN_PAUSE_S;
            }
        }
    }

    /// Jump to a route fraction (cancels any turn in progress).
    pub fn set_t(&mut self, t: f64) {
        self.t = t.clamp(0.0, 1.0);
        self.pause = 0.0;
    }

    /// Swing of the nose during the turn-around, radians (0 while flying, up to pi when done).
    pub fn yaw(&self) -> f64 {
        if self.pause <= 0.0 {
            return 0.0;
        }
        let p = 1.0 - self.pause / TURN_PAUSE_S; // 0 -> 1 over the pause
        PI * smooth((p - 0.25) / 0.6) // stand still, turn, stand still
    }

    pub fn state(&self, route: &Route) -> FlightState {
        route.state(self.t, self.outbound, self.yaw())
    }
}

/// Plane position (planet radii from the centre) and body axes in world space. The height above the
/// ground is multiplied by `exaggeration` so the aircraft visibly floats above the planet.
pub fn plane_frame(st: &FlightState, exaggeration: f64) -> (V3, V3, V3, V3) {
    let up_h = st.ground;
    let right = normalize(cross(st.forward, up_h));
    let (sp, cp) = (st.pitch.sin(), st.pitch.cos());
    let fwd = add(scale(st.forward, cp), scale(up_h, sp));
    let up = sub(scale(up_h, cp), scale(st.forward, sp));
    let pos = scale(st.ground, 1.0 + st.alt_m * exaggeration / EARTH_RADIUS_M);
    (pos, right, fwd, up)
}

/// Matrices for drawing a model at `size_mul` times its real size:
/// (model-view-projection, normal matrix mapping model normals to view space), column-major,
/// or `None` when the plane cannot be seen (behind the camera or hidden by the planet).
/// Everything is composed in f64 so there is no jitter however close the camera gets.
pub fn plane_matrices(
    cam: &Camera,
    aspect: f64,
    st: &FlightState,
    size_mul: f64,
    exaggeration: f64,
    radius_m: f64,
) -> Option<([f32; 16], [f32; 16])> {
    let (pos, right, fwd, up) = plane_frame(st, exaggeration);
    let s = size_mul / EARTH_RADIUS_M; // planet radii per metre
    let radius = radius_m * s;
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

    Some((crate::camera::cols_f32(&mvp), crate::camera::cols_f32(&nrm)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sfo_hnd() -> Route {
        Route::of(&AIRCRAFT[2])
    }

    #[test]
    fn routes_have_plausible_lengths_and_durations() {
        let hnd = sfo_hnd();
        let km = hnd.length_m() / 1000.0;
        assert!((8000.0..8500.0).contains(&km), "{km}");
        assert!((10.5..11.5).contains(&(hnd.duration_s() / 3600.0)));
        let (lo, la) = hnd.lonlat_deg(1.0);
        assert!((lo - 139.7798).abs() < 1e-6 && (la - 35.5494).abs() < 1e-6);
        // the great circle bulges north of both airports
        let (_, mid_lat) = hnd.lonlat_deg(0.5);
        assert!(mid_lat > 45.0, "{mid_lat}");

        let fra = Route::of(&AIRCRAFT[1]);
        let km = fra.length_m() / 1000.0;
        assert!((9000.0..9500.0).contains(&km), "SFO-FRA {km}");
        assert!((9.5..11.5).contains(&(fra.duration_s() / 3600.0)));

        let lbb = Route::of(&AIRCRAFT[0]);
        let km = lbb.length_m() / 1000.0;
        assert!((1100.0..1400.0).contains(&km), "LAS-LBB {km}");
        assert!((1.0..2.0).contains(&(lbb.duration_s() / 3600.0)));
    }

    #[test]
    fn altitude_profile_climbs_cruises_descends() {
        let r = sfo_hnd();
        assert_eq!(r.altitude_m(0.0), 0.0);
        assert!((r.altitude_m(0.5) - 11_582.0).abs() < 1e-6);
        assert!(r.altitude_m(0.01) > 100.0 && r.altitude_m(0.01) < 11_582.0);
        assert!(r.altitude_m(1.0) < 1e-6);
        assert!(r.state(0.01, true, 0.0).pitch > 0.05);
        assert!(r.state(0.99, true, 0.0).pitch < 0.0);
        assert!(r.state(0.5, true, 0.0).pitch.abs() < 1e-3);
        // flying back, the nose tips up near the far airport instead
        assert!(r.state(0.99, false, 0.0).pitch > 0.05);
        // a short hop still gets a full-height cruise only if long enough; never negative
        let hop = Route::of(&AIRCRAFT[0]);
        for i in 0..=100 {
            let a = hop.altitude_m(i as f64 / 100.0);
            assert!((0.0..=10_058.0 + 1e-6).contains(&a));
        }
    }

    #[test]
    fn returning_flies_the_other_way() {
        let r = Route::new((0.0, 0.0), (0.0, 10.0), 10_000.0, 220.0);
        let out = r.state(0.5, true, 0.0);
        let back = r.state(0.5, false, 0.0);
        assert!(dot(out.forward, back.forward) < -0.999);
        // half a turn at the end of the leg points the nose backwards
        let half = r.state(0.5, true, PI);
        assert!(dot(half.forward, back.forward) > 0.999);
        let quarter = r.state(0.5, true, PI / 2.0);
        assert!(dot(quarter.forward, out.forward).abs() < 1e-9);
        assert!((dot(quarter.forward, quarter.forward) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn ping_pong_pauses_turns_and_returns() {
        let mut sim = Sim::new(0.9);
        sim.playing = true;
        let leg = 100.0;
        // 0.1 of a leg at 1x takes 10 s; the next 1 s tick arrives at the far end
        for _ in 0..105 {
            sim.advance(0.1, 1.0, leg);
        }
        assert_eq!(sim.t, 1.0);
        assert!(sim.pause > 0.0 && sim.outbound, "waiting at the destination");
        assert!(sim.yaw() >= 0.0);
        // wait out the pause: it turns and heads back
        let mut waited = 0.0;
        while sim.pause > 0.0 && waited < 20.0 {
            sim.advance(0.1, 1.0, leg);
            waited += 0.1;
        }
        assert!((TURN_PAUSE_S - 1.0..TURN_PAUSE_S + 0.2).contains(&waited), "{waited}");
        assert!(!sim.outbound);
        for _ in 0..30 {
            sim.advance(0.1, 1.0, leg);
        }
        assert!(sim.t < 1.0 && sim.t > 0.95, "{}", sim.t);
        // all the way back, then outbound again, indefinitely
        for _ in 0..4000 {
            sim.advance(0.1, 60.0, leg);
        }
        assert!((0.0..=1.0).contains(&sim.t));
    }

    #[test]
    fn turn_swings_smoothly_from_zero_to_half_a_turn() {
        let mut sim = Sim::new(1.0);
        sim.playing = true;
        sim.pause = TURN_PAUSE_S;
        assert!(sim.yaw() < 1e-9);
        let mut prev = 0.0;
        for _ in 0..59 {
            sim.advance(0.1, 1.0, 100.0);
            let y = sim.yaw();
            assert!(y >= prev - 1e-12 && y <= PI + 1e-9);
            prev = y;
        }
        assert!(prev > 3.0, "{prev}");
        sim.set_t(0.5);
        assert_eq!(sim.pause, 0.0);
    }

    #[test]
    fn nose_points_up_the_screen_when_flying_north() {
        let r = Route::new((0.0, 0.0), (0.0, 10.0), 10_000.0, 220.0);
        let st = r.state(0.5, true, 0.0);
        let (lon, lat) = r.lonlat_deg(0.5);
        let cam = Camera { lon: lon.to_radians(), lat: lat.to_radians(), alt: 0.05, pitch: 0.0, heading: 0.0 };
        let (w, h) = (1000.0, 600.0);
        let (mvp, _n) = plane_matrices(&cam, w / h, &st, 100.0, 1.0, 21.0).expect("visible");
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
    fn exaggeration_lifts_the_plane() {
        let r = sfo_hnd();
        let st = r.state(0.5, true, 0.0);
        let (p1, ..) = plane_frame(&st, 1.0);
        let (p20, ..) = plane_frame(&st, 20.0);
        let h = |p: V3| dot(p, p).sqrt() - 1.0;
        assert!((h(p1) - 11_582.0 / EARTH_RADIUS_M).abs() < 1e-9);
        assert!((h(p20) / h(p1) - 20.0).abs() < 1e-6);
    }

    #[test]
    fn hidden_behind_planet_or_camera() {
        let r = Route::new((0.0, 0.0), (0.0, 10.0), 10_000.0, 220.0);
        let st = r.state(0.5, true, 0.0);
        // camera on the far side of the planet
        let far = Camera { lon: std::f64::consts::PI, lat: 0.0, alt: 0.5, pitch: 0.0, heading: 0.0 };
        assert!(plane_matrices(&far, 1.5, &st, 100.0, 1.0, 21.0).is_none());
        // camera lower than the plane, looking down: plane is behind it
        let low = Camera { lon: 0.0, lat: 5.0f64.to_radians(), alt: 0.0002, pitch: 0.0, heading: 0.0 };
        assert!(plane_matrices(&low, 1.5, &st, 1.0, 1.0, 21.0).is_none());
    }
}
