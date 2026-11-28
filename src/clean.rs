//! A tidied copy of a GPS stretch, for showing next to the original (which is never changed).
//! Phones between tall buildings jump around: a fix lands 30 m off, the next is back on the
//! street. Two simple steps take most of that out:
//!
//! 1. A point that sticks out sideways and comes straight back (a spike) is dropped.
//! 2. What is left is averaged with the few points on each side of it.
//!
//! The first and last points stay exactly where they were, and every kept point keeps its time, so
//! the cleaned track can be replayed like the original.

use crate::camera::{dot, normalize, scale, sub, V3};

const EARTH_M: f64 = 6_371_000.0;
/// A detour shorter than this is ordinary GPS noise, not a spike.
const SPIKE_MIN_M: f64 = 10.0;
/// A point is averaged with up to this many neighbours each way (per pass)...
const SMOOTH_STEPS: usize = 3;
/// ...but never with any further than this many metres along the path, so that widely spaced
/// fixes (driving) do not get their corners cut.
const SMOOTH_MAX_M: f64 = 40.0;
const PASSES: usize = 2;

/// Straight-line distance between two points on the globe, in metres (accurate for tiny steps).
pub fn dist_m(a: V3, b: V3) -> f64 {
    let d = sub(a, b);
    let chord = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    2.0 * (chord * 0.5).clamp(0.0, 1.0).asin() * EARTH_M
}

/// Drops the points that jump out and straight back; returns the indices of the kept points.
fn despike(pts: &[V3]) -> Vec<usize> {
    let n = pts.len();
    let mut keep: Vec<usize> = Vec::with_capacity(n);
    for i in 0..n {
        if i == 0 || i + 1 == n {
            keep.push(i);
            continue;
        }
        let prev = pts[*keep.last().unwrap_or(&0)];
        let detour = dist_m(prev, pts[i]) + dist_m(pts[i], pts[i + 1]);
        let direct = dist_m(prev, pts[i + 1]);
        // an interior angle under about 60 degrees, with legs long enough to matter
        if detour > SPIKE_MIN_M && detour > 2.0 * direct + 6.0 {
            continue;
        }
        keep.push(i);
    }
    keep
}

/// One pass of triangular averaging along the path.
fn smooth(pts: &[V3]) -> Vec<V3> {
    let n = pts.len();
    if n < 3 {
        return pts.to_vec();
    }
    let mut along = vec![0.0; n];
    for i in 1..n {
        along[i] = along[i - 1] + dist_m(pts[i - 1], pts[i]);
    }
    let mut out = Vec::with_capacity(n);
    out.push(pts[0]);
    let (mut lo, mut hi) = (0usize, 0usize);
    for i in 1..n - 1 {
        while i - lo > SMOOTH_STEPS || along[i] - along[lo] > SMOOTH_MAX_M {
            lo += 1;
        }
        hi = hi.max(i);
        while hi + 1 < n && hi + 1 - i <= SMOOTH_STEPS && along[hi + 1] - along[i] <= SMOOTH_MAX_M {
            hi += 1;
        }
        while hi > i && (hi - i > SMOOTH_STEPS || along[hi] - along[i] > SMOOTH_MAX_M) {
            hi -= 1;
        }
        let mut sum = [0.0; 3];
        let mut wsum = 0.0;
        for j in lo..=hi {
            let w = 1.0 - (j as f64 - i as f64).abs() / (SMOOTH_STEPS as f64 + 1.0);
            let q = scale(pts[j], w);
            for k in 0..3 {
                sum[k] += q[k];
            }
            wsum += w;
        }
        out.push(if wsum > 0.0 { normalize(sum) } else { pts[i] });
    }
    out.push(pts[n - 1]);
    out
}

/// The tidied points and their times (empty when `times` was empty).
pub fn clean(pts: &[V3], times: &[f64]) -> (Vec<V3>, Vec<f64>) {
    if pts.len() < 3 {
        return (pts.to_vec(), times.to_vec());
    }
    let keep = despike(pts);
    let kept: Vec<V3> = keep.iter().map(|&i| pts[i]).collect();
    let kept_t: Vec<f64> = if times.len() == pts.len() { keep.iter().map(|&i| times[i]).collect() } else { Vec::new() };
    let mut out = kept;
    for _ in 0..PASSES {
        out = smooth(&out);
    }
    // keep the first/last exactly (smoothing never moves them, this just guards the contract)
    debug_assert!(dot(out[0], pts[0]) > 0.999_999);
    (out, kept_t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::lonlat_to_vec;

    /// A point `east` metres east and `north` metres north of a spot in San Francisco.
    fn at(east: f64, north: f64) -> V3 {
        let (lon0, lat0) = (-122.40_f64, 37.78_f64);
        let lat = lat0 + north / 111_195.0;
        let lon = lon0 + east / (111_195.0 * lat0.to_radians().cos());
        lonlat_to_vec(lon.to_radians(), lat.to_radians())
    }

    fn length(p: &[V3]) -> f64 {
        p.windows(2).map(|w| dist_m(w[0], w[1])).sum()
    }

    #[test]
    fn a_spike_is_dropped_and_a_straight_walk_stays_straight() {
        // 100 m walk east with one fix thrown 40 m north
        let mut pts: Vec<V3> = (0..=20).map(|i| at(i as f64 * 5.0, 0.0)).collect();
        pts[10] = at(50.0, 40.0);
        let times: Vec<f64> = (0..=20).map(|i| i as f64 * 4.0).collect();
        let (c, t) = clean(&pts, &times);
        assert_eq!(c.len(), t.len());
        assert_eq!(c.len(), 20, "the spike is gone, its time with it");
        assert!(!t.contains(&40.0));
        // every kept point lies within a metre of the true line (north = 0)
        for (p, _) in c.iter().zip(&t) {
            let d = (0..=100).map(|e| dist_m(*p, at(e as f64, 0.0))).fold(f64::INFINITY, f64::min);
            assert!(d < 1.0, "{d} m off the line");
        }
    }

    #[test]
    fn noise_is_reduced_and_the_ends_stay_put() {
        // a 300 m walk north, a fix every 3 m, each thrown up to 10 m sideways (fixed pseudo-random)
        let mut seed = 12345u32;
        let mut noise = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((seed >> 8) as f64 / (1u32 << 24) as f64 - 0.5) * 20.0
        };
        let pts: Vec<V3> = (0..=100).map(|i| at(noise(), i as f64 * 3.0)).collect();
        let (c, _) = clean(&pts, &[]);
        assert!(dist_m(c[0], pts[0]) < 1e-6 && dist_m(*c.last().unwrap(), *pts.last().unwrap()) < 1e-6);
        // sideways distance from the true line (east = 0) away from the ends
        let side = |p: &V3| (0..=3000).map(|n| dist_m(*p, at(0.0, n as f64 / 10.0))).fold(f64::INFINITY, f64::min);
        let rms = |v: &[V3]| (v.iter().map(|p| side(p).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
        let before = rms(&pts[20..80]);
        let after = rms(&c[c.len() / 5..c.len() * 4 / 5]);
        assert!(after < before * 0.6, "{after} vs {before}");
    }

    #[test]
    fn a_proper_corner_survives() {
        // walk 60 m east then 60 m north: a real 90 degree street corner, one point per 3 m
        let mut pts: Vec<V3> = (0..=20).map(|i| at(i as f64 * 3.0, 0.0)).collect();
        pts.extend((1..=20).map(|i| at(60.0, i as f64 * 3.0)));
        let (c, _) = clean(&pts, &[]);
        assert_eq!(c.len(), pts.len(), "nothing dropped at a corner");
        let corner = c.iter().map(|p| dist_m(*p, at(60.0, 0.0))).fold(f64::INFINITY, f64::min);
        assert!(corner < 8.0, "{corner}");
        assert!(length(&c) > 100.0, "{}", length(&c));
    }

    #[test]
    fn short_inputs_pass_through() {
        let (c, t) = clean(&[at(0.0, 0.0), at(1.0, 1.0)], &[1.0, 2.0]);
        assert_eq!((c.len(), t), (2, vec![1.0, 2.0]));
        assert!(clean(&[], &[]).0.is_empty());
    }
}
