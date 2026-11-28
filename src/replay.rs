//! The clock of a GPS track: where the walker was after so many seconds, how far they had gone and
//! how fast. Pure maths (no drawing), so the replay widget stays simple.
//!
//! A track recorded with pauses is several stretches. The replay skips the pauses: `elapsed` counts
//! only the time spent moving along stretches, and the position jumps from the end of one stretch to
//! the start of the next.

use crate::camera::{add, normalize, scale, V3};
use crate::clean::dist_m;

/// Walking pace used when a file has no times at all (about 5 km/h).
const ASSUMED_M_S: f64 = 1.4;
/// Speed at a point is measured over this many seconds each side of it.
const SPEED_WINDOW_S: f64 = 6.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Timing {
    /// Every point has its own time: speeds are real.
    Exact,
    /// Only the overall duration is known (from the track's name): constant speed along the way.
    Estimated,
    /// No times at all: the replay assumes a walking pace and shows no speed.
    None,
}

pub struct Timeline {
    pub pts: Vec<V3>,
    /// Seconds spent moving, at each point (the gap between two stretches adds nothing).
    active: Vec<f64>,
    /// Metres from the start, at each point (a gap between two stretches adds nothing).
    dist: Vec<f64>,
    /// Metres per second at each point (smoothed over a few seconds); all zero for `Timing::None`.
    speed: Vec<f64>,
    /// True for the first point of every stretch.
    first_of_stretch: Vec<bool>,
    pub timing: Timing,
}

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub pos: V3,
    pub elapsed: f64,
    pub dist_m: f64,
    pub speed_ms: f64,
}

impl Timeline {
    /// `times[i]` is empty or one time (seconds) per point of `segs[i]`. `span` is the track's
    /// duration when only that is known.
    pub fn new(segs: &[Vec<V3>], times: &[Vec<f64>], span: Option<f64>) -> Timeline {
        let mut pts = Vec::new();
        let mut first = Vec::new();
        let mut dist = Vec::new();
        let mut clock: Vec<f64> = Vec::new();
        let all_timed = !segs.is_empty() && segs.iter().zip(times).all(|(s, t)| t.len() == s.len()) && times.len() == segs.len();
        let mut d = 0.0;
        let mut ends: Vec<(usize, usize)> = Vec::new();
        for (k, s) in segs.iter().enumerate() {
            let start = pts.len();
            for (i, &p) in s.iter().enumerate() {
                if i > 0 {
                    d += dist_m(s[i - 1], p);
                }
                pts.push(p);
                first.push(i == 0);
                dist.push(d);
                if all_timed {
                    clock.push(times[k][i]);
                }
            }
            if !s.is_empty() {
                ends.push((start, pts.len()));
            }
        }
        let total_d = d;
        let mut active = vec![0.0; pts.len()];
        let mut timing = Timing::None;
        if all_timed {
            let mut a = 0.0;
            for i in 0..pts.len() {
                if i > 0 && !first[i] {
                    a += (clock[i] - clock[i - 1]).max(0.0);
                }
                active[i] = a;
            }
            if a > 0.0 {
                timing = Timing::Exact;
            }
        }
        if timing != Timing::Exact {
            match span {
                Some(s) if s > 0.0 && total_d > 0.0 => {
                    timing = Timing::Estimated;
                    for i in 0..pts.len() {
                        active[i] = dist[i] / total_d * s;
                    }
                }
                _ => {
                    for i in 0..pts.len() {
                        active[i] = dist[i] / ASSUMED_M_S;
                    }
                }
            }
        }
        let total_a = active.last().copied().unwrap_or(0.0);
        let mut speed = vec![0.0; pts.len()];
        match timing {
            Timing::Exact => {
                for &(s0, s1) in &ends {
                    let a = &active[s0..s1];
                    for i in 0..a.len() {
                        let lo = a.partition_point(|&x| x < a[i] - SPEED_WINDOW_S);
                        let hi = a.partition_point(|&x| x <= a[i] + SPEED_WINDOW_S).saturating_sub(1);
                        let dt = a[hi] - a[lo];
                        speed[s0 + i] = if dt > 0.5 { (dist[s0 + hi] - dist[s0 + lo]) / dt } else { 0.0 };
                    }
                }
            }
            Timing::Estimated => {
                if total_a > 0.0 {
                    speed.iter_mut().for_each(|v| *v = total_d / total_a);
                }
            }
            Timing::None => {}
        }
        Timeline { pts, active, dist, speed, first_of_stretch: first, timing }
    }

    pub fn is_empty(&self) -> bool {
        self.pts.len() < 2
    }

    /// Seconds of moving, start to end.
    pub fn total_secs(&self) -> f64 {
        self.active.last().copied().unwrap_or(0.0)
    }

    pub fn total_m(&self) -> f64 {
        self.dist.last().copied().unwrap_or(0.0)
    }

    /// Average speed while moving, metres per second.
    pub fn avg_ms(&self) -> f64 {
        let t = self.total_secs();
        if t > 0.0 {
            self.total_m() / t
        } else {
            0.0
        }
    }

    pub fn max_ms(&self) -> f64 {
        self.speed.iter().cloned().fold(0.0, f64::max)
    }

    /// Where the walker is `u` seconds into the walk (clamped to the ends).
    pub fn at(&self, u: f64) -> Sample {
        let n = self.pts.len();
        if n == 0 {
            return Sample { pos: [1.0, 0.0, 0.0], elapsed: 0.0, dist_m: 0.0, speed_ms: 0.0 };
        }
        let u = if u.is_finite() { u.clamp(0.0, self.total_secs()) } else { 0.0 };
        // first point strictly after `u`
        let k = self.active.partition_point(|&x| x <= u);
        if k == 0 || k >= n {
            let i = if k == 0 { 0 } else { n - 1 };
            return Sample { pos: self.pts[i], elapsed: u, dist_m: self.dist[i], speed_ms: self.speed[i] };
        }
        let i = k - 1;
        let span = self.active[k] - self.active[i];
        let f = if span > 1e-9 && !self.first_of_stretch[k] { ((u - self.active[i]) / span).clamp(0.0, 1.0) } else { 0.0 };
        let pos = normalize(add(scale(self.pts[i], 1.0 - f), scale(self.pts[k], f)));
        Sample {
            pos,
            elapsed: u,
            dist_m: self.dist[i] + (self.dist[k] - self.dist[i]) * f,
            speed_ms: self.speed[i] + (self.speed[k] - self.speed[i]) * f,
        }
    }
}

// ------------------------------------------------------------------ text for the widget

/// "2:05" or "1:02:05".
pub fn hms(secs: f64) -> String {
    let s = if secs.is_finite() { secs.max(0.0).round() as u64 } else { 0 };
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// A distance in metres as "350 m" / "1.24 km", and in feet and miles as "1150 ft" / "0.77 mi".
pub fn distance_text(m: f64) -> (String, String) {
    let metric = if m < 1000.0 { format!("{:.0} m", m) } else { format!("{:.2} km", m / 1000.0) };
    let feet = m * 3.280_84;
    let imperial = if feet < 1000.0 { format!("{:.0} ft", feet) } else { format!("{:.2} mi", m / 1609.344) };
    (metric, imperial)
}

/// A speed in metres per second as ("5.1", "3.2 mph"); the first is in km/h.
pub fn speed_text(ms: f64) -> (String, String) {
    (format!("{:.1}", ms * 3.6), format!("{:.1} mph", ms * 2.236_94))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::lonlat_to_vec;

    fn at(east: f64, north: f64) -> V3 {
        let (lon0, lat0) = (-122.40_f64, 37.78_f64);
        let lat = lat0 + north / 111_195.0;
        let lon = lon0 + east / (111_195.0 * lat0.to_radians().cos());
        lonlat_to_vec(lon.to_radians(), lat.to_radians())
    }

    /// 10 points, 7.5 m apart, every 5 s (1.5 m/s), then a pause, then 10 more at 3 m/s.
    fn two_stretches() -> (Vec<Vec<V3>>, Vec<Vec<f64>>) {
        let a: Vec<V3> = (0..10).map(|i| at(i as f64 * 7.5, 0.0)).collect();
        let ta: Vec<f64> = (0..10).map(|i| 1000.0 + i as f64 * 5.0).collect();
        let b: Vec<V3> = (0..10).map(|i| at(500.0 + i as f64 * 15.0, 200.0)).collect();
        let tb: Vec<f64> = (0..10).map(|i| 5000.0 + i as f64 * 5.0).collect();
        (vec![a, b], vec![ta, tb])
    }

    #[test]
    fn exact_times_give_real_speeds_and_skip_pauses() {
        let (segs, times) = two_stretches();
        let t = Timeline::new(&segs, &times, None);
        assert_eq!(t.timing, Timing::Exact);
        // 9 steps of 5 s in each stretch; the hour-long pause is not counted
        assert!((t.total_secs() - 90.0).abs() < 1e-9);
        assert!((t.total_m() - (9.0 * 7.5 + 9.0 * 15.0)).abs() < 0.5, "{}", t.total_m());
        let s = t.at(20.0);
        assert!((s.speed_ms - 1.5).abs() < 0.1, "{}", s.speed_ms);
        assert!((s.dist_m - 30.0).abs() < 0.5, "{}", s.dist_m);
        let s = t.at(70.0);
        assert!((s.speed_ms - 3.0).abs() < 0.1, "{}", s.speed_ms);
        assert!(t.max_ms() > 2.9 && t.max_ms() < 3.1);
        // just before 45 s the first stretch has ended; a moment later we are at the start of the second
        let end_a = t.at(44.99).pos;
        let start_b = t.at(45.5).pos;
        assert!(dist_m(end_a, at(67.5, 0.0)) < 0.5);
        assert!(dist_m(start_b, at(500.0, 200.0)) < 3.0, "{}", dist_m(start_b, at(500.0, 200.0)));
        // out of range is clamped
        assert!(dist_m(t.at(-5.0).pos, at(0.0, 0.0)) < 0.01);
        assert!(dist_m(t.at(1e9).pos, at(500.0 + 135.0, 200.0)) < 0.01);
    }

    #[test]
    fn numbers_read_well() {
        assert_eq!(hms(125.0), "2:05");
        assert_eq!(hms(3725.4), "1:02:05");
        assert_eq!(hms(-3.0), "0:00");
        assert_eq!(hms(f64::NAN), "0:00");
        assert_eq!(distance_text(250.0), ("250 m".to_string(), "820 ft".to_string()));
        assert_eq!(distance_text(350.0).1, "0.22 mi");
        assert_eq!(distance_text(1240.0), ("1.24 km".to_string(), "0.77 mi".to_string()));
        assert_eq!(speed_text(1.5), ("5.4".to_string(), "3.4 mph".to_string()));
    }

    #[test]
    fn position_is_interpolated_between_fixes() {
        let (segs, times) = two_stretches();
        let t = Timeline::new(&segs, &times, None);
        let s = t.at(2.5);
        assert!(dist_m(s.pos, at(3.75, 0.0)) < 0.05);
        assert!((s.dist_m - 3.75).abs() < 0.05);
    }

    #[test]
    fn a_name_span_gives_an_estimate_and_nothing_gives_a_walking_pace() {
        let (segs, _) = two_stretches();
        let none: Vec<Vec<f64>> = vec![Vec::new(), Vec::new()];
        let t = Timeline::new(&segs, &none, Some(600.0));
        assert_eq!(t.timing, Timing::Estimated);
        assert!((t.total_secs() - 600.0).abs() < 1e-9);
        assert!((t.at(300.0).speed_ms - t.total_m() / 600.0).abs() < 1e-9);
        let t = Timeline::new(&segs, &none, None);
        assert_eq!(t.timing, Timing::None);
        assert!((t.total_secs() - t.total_m() / ASSUMED_M_S).abs() < 1e-6);
        assert_eq!(t.max_ms(), 0.0);
    }

    #[test]
    fn partly_timed_files_are_not_trusted() {
        let (segs, mut times) = two_stretches();
        times[1].clear();
        assert_ne!(Timeline::new(&segs, &times, None).timing, Timing::Exact);
        assert!(Timeline::new(&[], &[], None).is_empty());
        assert_eq!(Timeline::new(&[], &[], None).at(3.0).elapsed, 0.0);
    }
}
