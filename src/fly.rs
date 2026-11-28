//! The smooth camera flight between two views (used by "go to city", the Places list and the
//! route buttons).

use crate::camera::{self, dot, Camera, V3};

/// A smooth camera move between two views, following van Wijk & Nuij's "smooth and efficient zooming
/// and panning": the path that keeps the apparent speed on screen constant. It climbs just as far
/// as the distance needs, glides across, and comes down mirror-fashion, so the approach to the
/// destination is as gentle as the departure.
pub struct Fly {
    pub from: Camera,
    pub to: Camera,
    pub start: f64,
    pub duration: f64,
    /// Ground distance (radians) and the view width at the start and end, in the same units.
    d: f64,
    w0: f64,
    w1: f64,
    r0: f64,
    /// Length of the path in the algorithm's own units.
    total: f64,
}

/// The zoom-and-pan "curvature": how far the path climbs (larger = higher, more dramatic).
const FLY_RHO: f64 = std::f64::consts::SQRT_2;

/// How long a flight takes: (name, seconds per unit of path length; 0 = jump straight there).
pub const FLY_SPEEDS: [(&str, f64); 4] = [("Slow", 0.95), ("Normal", 0.62), ("Fast", 0.36), ("Jump", 0.0)];

/// Ground width of the view as a multiple of the camera height (a wide window).
fn view_width_per_alt() -> f64 {
    2.0 * (camera::FOV_Y * 0.5).tan() * 1.6
}

impl Fly {
    pub fn new(from: Camera, to: Camera, start: f64, min_duration: f64, secs_per_unit: f64) -> Fly {
        let d = dot(from.dir(), to.dir()).clamp(-1.0, 1.0).acos();
        let k = view_width_per_alt();
        let (w0, w1) = (from.alt * k, to.alt * k);
        let rho = FLY_RHO;
        let (r0, total) = if d < 1e-7 {
            // no ground to cover: only zoom
            (0.0, (w1 / w0).ln().abs() / rho)
        } else {
            let rho2 = rho * rho;
            let b0 = (w1 * w1 - w0 * w0 + rho2 * rho2 * d * d) / (2.0 * w0 * rho2 * d);
            let b1 = (w1 * w1 - w0 * w0 - rho2 * rho2 * d * d) / (2.0 * w1 * rho2 * d);
            // ln(sqrt(b^2 + 1) - b) written as -asinh(b), which stays accurate for large b
            let (r0, r1) = (-b0.asinh(), -b1.asinh());
            (r0, (r1 - r0) / rho)
        };
        // a short hop still takes a moment, a long one as long as its path needs (never forever)
        let duration = (total * secs_per_unit).max(min_duration * 0.45).min(11.0);
        Fly { from, to, start, duration, d, w0, w1, r0, total }
    }

    /// (fraction of the ground distance covered, view width) after path length `s`.
    fn at(&self, s: f64) -> (f64, f64) {
        let rho = FLY_RHO;
        if self.d < 1e-7 {
            let f = if self.total > 0.0 { s / self.total } else { 1.0 };
            return (f, self.w0 * (self.w1 / self.w0).powf(f));
        }
        let (c0, s0) = (self.r0.cosh(), self.r0.sinh());
        let arg = rho * s + self.r0;
        let u = self.w0 / (rho * rho) * (c0 * arg.tanh() - s0);
        let w = self.w0 * c0 / arg.cosh();
        ((u / self.d).clamp(0.0, 1.0), w)
    }

    /// The camera `t` (0..1) of the way through.
    pub fn camera(&self, t: f64) -> Camera {
        // a gentle ease in and out on top of the constant-speed path
        let smooth = t * t * (3.0 - 2.0 * t);
        let te = 0.5 * t + 0.5 * smooth;
        let (f, w) = self.at(te * self.total);
        // along the great circle
        let (a, b) = (self.from.dir(), self.to.dir());
        let p = if self.d < 1e-7 {
            a
        } else {
            let (sa, sb) = (((1.0 - f) * self.d).sin(), (f * self.d).sin());
            normalize_v([(a[0] * sa + b[0] * sb), (a[1] * sa + b[1] * sb), (a[2] * sa + b[2] * sb)])
        };
        let (lon, lat) = camera::vec_to_lonlat(p);
        // never so high that the globe shrinks to a dot
        let alt = (w / view_width_per_alt()).min(GLOBE_ALT_MAX.max(self.from.alt).max(self.to.alt));
        let e = smooth;
        let mut c = Camera {
            lon,
            lat,
            alt,
            pitch: self.from.pitch + (self.to.pitch - self.from.pitch) * e,
            heading: self.from.heading + camera::wrap_pi(self.to.heading - self.from.heading) * e,
            shift: [
                self.from.shift[0] + (self.to.shift[0] - self.from.shift[0]) * e,
                self.from.shift[1] + (self.to.shift[1] - self.from.shift[1]) * e,
            ],
        };
        c.normalize();
        c
    }
}

/// The highest a flight climbs (planet radii): the whole globe comfortably in view.
const GLOBE_ALT_MAX: f64 = 2.6;

fn normalize_v(a: V3) -> V3 {
    camera::normalize(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam(lon: f64, lat: f64, alt: f64) -> Camera {
        Camera::from_degrees(lon, lat, alt)
    }

    fn check(from: Camera, to: Camera) {
        let f = Fly::new(from, to, 0.0, 3.0, 0.62);
        assert!(f.duration > 0.5 && f.duration <= 11.0, "duration {}", f.duration);
        let lo = from.alt.min(to.alt) * 0.999;
        let mut prev = f.camera(0.0);
        assert!((prev.alt - from.alt).abs() / from.alt < 1e-6, "start alt");
        assert!(dot(prev.dir(), from.dir()) > 1.0 - 1e-9, "start pos");
        let end = f.camera(1.0);
        assert!((end.alt - to.alt).abs() / to.alt < 1e-6, "end alt {} vs {}", end.alt, to.alt);
        assert!(dot(end.dir(), to.dir()) > 1.0 - 1e-9, "end pos");
        let mut peak: f64 = 0.0;
        for i in 1..=400 {
            let c = f.camera(i as f64 / 400.0);
            assert!(c.alt >= lo, "dips below both ends: {}", c.alt);
            assert!(c.alt.is_finite() && c.lon.is_finite() && c.lat.is_finite());
            // no jumps: neither the log-altitude nor the position leaps between samples
            assert!((c.alt / prev.alt).ln().abs() < 0.2, "altitude leap at {i}");
            // ... and the ground never slides more than a tenth of what the view shows per sample
            let step = dot(c.dir(), prev.dir()).clamp(-1.0, 1.0).acos();
            assert!(step < 0.1 * c.alt.max(prev.alt) * view_width_per_alt() + 1e-9, "position leap at {i}: {step}");
            peak = peak.max(c.alt);
            prev = c;
        }
        assert!(peak >= lo);
    }

    #[test]
    fn flights_start_and_end_exactly_and_stay_smooth() {
        check(cam(-122.4, 37.8, 0.0005), cam(139.7, 35.7, 0.0005)); // across the Pacific
        check(cam(2.35, 48.85, 0.001), cam(2.4, 48.9, 0.001)); // a short hop
        check(cam(0.0, 0.0, 2.0), cam(10.0, 10.0, 0.0003)); // from the globe to a city
        check(cam(10.0, 10.0, 0.0003), cam(10.0, 10.0, 2.0)); // straight out
        check(cam(10.0, 10.0, 0.001), cam(10.0, 10.0, 0.001)); // nowhere
        check(cam(0.0, 0.0, 0.001), cam(179.9, 0.0, 0.001)); // half way round
    }

    #[test]
    fn long_flights_climb_and_short_ones_barely_do() {
        let far = Fly::new(cam(-122.4, 37.8, 0.0005), cam(139.7, 35.7, 0.0005), 0.0, 3.0, 0.62);
        let near = Fly::new(cam(2.35, 48.85, 0.001), cam(2.36, 48.86, 0.001), 0.0, 3.0, 0.62);
        let top = |f: &Fly| (0..=200).map(|i| f.camera(i as f64 / 200.0).alt).fold(0.0, f64::max);
        assert!(top(&far) > 0.5, "ocean crossing climbs high: {}", top(&far));
        assert!(top(&near) < 0.02, "a short hop stays low: {}", top(&near));
        // the descent is the mirror of the climb: halfway up as late as it was early
        let a = far.camera(0.25).alt;
        let b = far.camera(0.75).alt;
        assert!((a.ln() - b.ln()).abs() < 1.0, "asymmetric: {a} vs {b}");
    }
}
