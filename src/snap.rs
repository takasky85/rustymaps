//! Snaps a GPS track onto the street network ("map matching"), with no outside data: the streets
//! are the road lines already in the map tiles.
//!
//! Every fix gets a few candidate spots on nearby streets. A hidden-Markov model then picks the most
//! believable chain of them: close to the fixes, and with a walking distance along the streets
//! between neighbours that matches the distance between the fixes themselves (so the chain does not
//! jump to a parallel street and back). Fixes with no street close by (a park, a plaza) are left
//! where they were, and the streets' own corners are inserted between snapped fixes so a turn at a
//! junction is a proper corner.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

use crate::camera::{lonlat_to_vec, vec_to_lonlat, V3};

const EARTH_M: f64 = 6_371_000.0;
/// A fix is only snapped to a street at most this far away.
const RADIUS_M: f64 = 30.0;
/// How far a fix is expected to be off, typically (metres).
const SIGMA_M: f64 = 15.0;
/// How much a mismatch between walked and straight-line distance costs (metres per unit).
const BETA_M: f64 = 8.0;
/// Candidates kept per fix.
const MAX_CANDIDATES: usize = 8;
const CELL_M: f64 = 40.0;

/// Metres east and north of a spot, for a small area.
#[derive(Clone, Copy)]
struct Plane {
    lon0: f64,
    lat0: f64,
    kx: f64,
}

impl Plane {
    fn new(center: V3) -> Plane {
        let (lon0, lat0) = vec_to_lonlat(center);
        Plane { lon0, lat0, kx: lat0.cos().max(0.05) }
    }

    fn xy(&self, p: V3) -> [f64; 2] {
        let (lon, lat) = vec_to_lonlat(p);
        let mut dl = lon - self.lon0;
        if dl > std::f64::consts::PI {
            dl -= 2.0 * std::f64::consts::PI;
        } else if dl < -std::f64::consts::PI {
            dl += 2.0 * std::f64::consts::PI;
        }
        [dl * self.kx * EARTH_M, (lat - self.lat0) * EARTH_M]
    }

    fn back(&self, xy: [f64; 2]) -> V3 {
        lonlat_to_vec(self.lon0 + xy[0] / (self.kx * EARTH_M), self.lat0 + xy[1] / EARTH_M)
    }
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

struct Seg {
    a: usize,
    b: usize,
    len: f64,
}

/// The street network around a track.
struct Roads {
    nodes: Vec<[f64; 2]>,
    adj: Vec<Vec<(usize, f64)>>,
    segs: Vec<Seg>,
    grid: HashMap<(i32, i32), Vec<usize>>,
}

impl Roads {
    fn new(lines: &[Vec<[f64; 2]>]) -> Roads {
        let mut r = Roads { nodes: Vec::new(), adj: Vec::new(), segs: Vec::new(), grid: HashMap::new() };
        let mut ids: HashMap<(i64, i64), usize> = HashMap::new();
        let mut seen: HashSet<(usize, usize)> = HashSet::new();
        for line in lines {
            let mut prev: Option<usize> = None;
            for &p in line {
                // vertices closer than half a metre are one junction (also joins tile edges)
                let key = ((p[0] * 2.0).round() as i64, (p[1] * 2.0).round() as i64);
                let id = *ids.entry(key).or_insert_with(|| {
                    r.nodes.push(p);
                    r.adj.push(Vec::new());
                    r.nodes.len() - 1
                });
                if let Some(a) = prev {
                    if a != id && seen.insert((a.min(id), a.max(id))) {
                        let len = dist(r.nodes[a], r.nodes[id]);
                        let k = r.segs.len();
                        r.segs.push(Seg { a, b: id, len });
                        r.adj[a].push((id, len));
                        r.adj[id].push((a, len));
                        let (pa, pb) = (r.nodes[a], r.nodes[id]);
                        let (x0, x1) = ((pa[0].min(pb[0]) / CELL_M).floor() as i32, (pa[0].max(pb[0]) / CELL_M).floor() as i32);
                        let (y0, y1) = ((pa[1].min(pb[1]) / CELL_M).floor() as i32, (pa[1].max(pb[1]) / CELL_M).floor() as i32);
                        // an absurdly long segment would fill thousands of cells: it cannot be near a walk
                        if (x1 - x0 + 1) as i64 * (y1 - y0 + 1) as i64 <= 400 {
                            for cx in x0..=x1 {
                                for cy in y0..=y1 {
                                    r.grid.entry((cx, cy)).or_default().push(k);
                                }
                            }
                        }
                    }
                }
                prev = Some(id);
            }
        }
        r
    }

    /// The nearest spot on each street within `radius` of `p`, nearest first.
    fn candidates(&self, p: [f64; 2]) -> Vec<Cand> {
        let (x0, x1) = (((p[0] - RADIUS_M) / CELL_M).floor() as i32, ((p[0] + RADIUS_M) / CELL_M).floor() as i32);
        let (y0, y1) = (((p[1] - RADIUS_M) / CELL_M).floor() as i32, ((p[1] + RADIUS_M) / CELL_M).floor() as i32);
        let mut seen: HashSet<usize> = HashSet::new();
        let mut out: Vec<Cand> = Vec::new();
        for cx in x0..=x1 {
            for cy in y0..=y1 {
                let Some(list) = self.grid.get(&(cx, cy)) else { continue };
                for &k in list {
                    if !seen.insert(k) {
                        continue;
                    }
                    let s = &self.segs[k];
                    let (a, b) = (self.nodes[s.a], self.nodes[s.b]);
                    let ab = [b[0] - a[0], b[1] - a[1]];
                    let l2 = ab[0] * ab[0] + ab[1] * ab[1];
                    let t = if l2 <= 1e-9 { 0.0 } else { (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / l2).clamp(0.0, 1.0) };
                    let at = [a[0] + ab[0] * t, a[1] + ab[1] * t];
                    let d = dist(p, at);
                    if d <= RADIUS_M {
                        out.push(Cand { seg: k, t, at, d });
                    }
                }
            }
        }
        out.sort_by(|x, y| x.d.partial_cmp(&y.d).unwrap_or(Ordering::Equal));
        out.truncate(MAX_CANDIDATES);
        out
    }

    /// Shortest distances (and the way back) over the streets from a spot on a segment, to every
    /// junction within `limit` metres.
    fn spread(&self, from: &Cand, limit: f64) -> Spread {
        let s = &self.segs[from.seg];
        let mut best: HashMap<usize, f64> = HashMap::new();
        let mut parent: HashMap<usize, usize> = HashMap::new();
        let mut heap: BinaryHeap<Item> = BinaryHeap::new();
        for (node, d) in [(s.a, from.t * s.len), (s.b, (1.0 - from.t) * s.len)] {
            if d <= limit && best.get(&node).map_or(true, |&o| d < o) {
                best.insert(node, d);
                heap.push(Item { d, node });
            }
        }
        while let Some(Item { d, node }) = heap.pop() {
            if best.get(&node).map_or(false, |&o| d > o) {
                continue;
            }
            for &(nb, len) in &self.adj[node] {
                let nd = d + len;
                if nd <= limit && best.get(&nb).map_or(true, |&o| nd < o) {
                    best.insert(nb, nd);
                    parent.insert(nb, node);
                    heap.push(Item { d: nd, node: nb });
                }
            }
        }
        Spread { best, parent }
    }

    /// Walking distance from the spot a spread started at to the spot `to`, and the junctions passed
    /// (empty when both are on one segment). `None` when out of reach.
    fn route(&self, from: &Cand, spread: &Spread, to: &Cand) -> Option<(f64, Vec<usize>)> {
        if from.seg == to.seg {
            let len = self.segs[from.seg].len;
            return Some(((to.t - from.t).abs() * len, Vec::new()));
        }
        let s = &self.segs[to.seg];
        let mut best: Option<(f64, usize)> = None;
        for (node, off) in [(s.a, to.t * s.len), (s.b, (1.0 - to.t) * s.len)] {
            if let Some(&d) = spread.best.get(&node) {
                if best.map_or(true, |b| d + off < b.0) {
                    best = Some((d + off, node));
                }
            }
        }
        let (total, last) = best?;
        let mut path = vec![last];
        let mut cur = last;
        while let Some(&p) = spread.parent.get(&cur) {
            path.push(p);
            cur = p;
        }
        path.reverse();
        Some((total, path))
    }
}

struct Spread {
    best: HashMap<usize, f64>,
    parent: HashMap<usize, usize>,
}

#[derive(Clone, Copy)]
struct Cand {
    seg: usize,
    t: f64,
    at: [f64; 2],
    d: f64,
}

struct Item {
    d: f64,
    node: usize,
}
impl PartialEq for Item {
    fn eq(&self, o: &Self) -> bool {
        self.d == o.d
    }
}
impl Eq for Item {}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Item {
    // reversed, so the heap gives the smallest distance first
    fn cmp(&self, o: &Self) -> Ordering {
        o.d.partial_cmp(&self.d).unwrap_or(Ordering::Equal)
    }
}

pub struct Snapped {
    pub pts: Vec<V3>,
    /// One time per point when the input had times, else empty.
    pub times: Vec<f64>,
    /// How many of the input fixes ended up on a street.
    pub matched: usize,
    pub total: usize,
}

/// Snaps one stretch of a track. `roads` are street polylines as points on the globe.
pub fn snap(points: &[V3], times: &[f64], roads: &[Vec<V3>]) -> Snapped {
    let n = points.len();
    let unchanged = || Snapped { pts: points.to_vec(), times: times.to_vec(), matched: 0, total: n };
    if n < 2 || roads.is_empty() {
        return unchanged();
    }
    let timed = times.len() == n;
    let plane = Plane::new(points[n / 2]);
    let lines: Vec<Vec<[f64; 2]>> = roads.iter().map(|l| l.iter().map(|&p| plane.xy(p)).collect()).collect();
    let net = Roads::new(&lines);
    if net.segs.is_empty() {
        return unchanged();
    }
    let xy: Vec<[f64; 2]> = points.iter().map(|&p| plane.xy(p)).collect();

    // --- candidates, and the cheapest chain through them (Viterbi) ---------------------------
    let cands: Vec<Vec<Cand>> = xy.iter().map(|&p| net.candidates(p)).collect();
    let emis = |c: &Cand| 0.5 * (c.d / SIGMA_M).powi(2);
    let mut cost: Vec<Vec<f64>> = vec![Vec::new(); n];
    let mut back: Vec<Vec<Option<usize>>> = vec![Vec::new(); n];
    // for the last fix of a finished chain, the candidate that ends it
    let mut ends: HashMap<usize, usize> = HashMap::new();
    let argmin = |v: &[f64]| v.iter().enumerate().min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(Ordering::Equal)).map(|(i, _)| i);
    let mut last: Option<usize> = None; // the previous fix that had candidates
    for i in 0..n {
        if cands[i].is_empty() {
            if let Some(l) = last.take() {
                if let Some(k) = argmin(&cost[l]) {
                    ends.insert(l, k);
                }
            }
            continue;
        }
        let mut c = vec![f64::INFINITY; cands[i].len()];
        let mut bk: Vec<Option<usize>> = vec![None; cands[i].len()];
        let mut any_link = false;
        if let Some(l) = last {
            let line = dist(xy[l], xy[i]);
            let limit = (line * 3.0 + 40.0).max(80.0);
            for (a, ca) in cands[l].iter().enumerate() {
                let spread = net.spread(ca, limit);
                for (b, cb) in cands[i].iter().enumerate() {
                    if let Some((walk, _)) = net.route(ca, &spread, cb) {
                        let v = cost[l][a] + (walk - line).abs() / BETA_M + emis(cb);
                        if v < c[b] {
                            c[b] = v;
                            bk[b] = Some(a);
                            any_link = true;
                        }
                    }
                }
            }
            if !any_link {
                // nothing is reachable: the chain ends here and a new one starts
                if let Some(k) = argmin(&cost[l]) {
                    ends.insert(l, k);
                }
            }
        }
        for (b, cb) in cands[i].iter().enumerate() {
            if !c[b].is_finite() {
                c[b] = emis(cb);
                bk[b] = None;
            }
        }
        cost[i] = c;
        back[i] = bk;
        last = Some(i);
    }
    if let Some(l) = last {
        if let Some(k) = argmin(&cost[l]) {
            ends.insert(l, k);
        }
    }

    // --- walk the chains backwards to get each fix's chosen candidate --------------------------
    let mut chosen: Vec<Option<usize>> = vec![None; n];
    let mut i = n;
    let mut cur: Option<usize> = None;
    while i > 0 {
        i -= 1;
        if cands[i].is_empty() {
            cur = None;
            continue;
        }
        let k = match cur {
            Some(k) => k,
            None => match ends.get(&i) {
                Some(&k) => k,
                None => match argmin(&cost[i]) {
                    Some(k) => k,
                    None => continue,
                },
            },
        };
        chosen[i] = Some(k);
        cur = back[i][k];
    }

    // --- build the output: snapped fixes with the streets' corners between them ----------------
    let mut pts: Vec<V3> = Vec::with_capacity(n + n / 4);
    let mut out_t: Vec<f64> = Vec::new();
    let mut matched = 0;
    let mut prev_matched: Option<usize> = None;
    let push = |pts: &mut Vec<V3>, out_t: &mut Vec<f64>, p: V3, t: f64| {
        pts.push(p);
        if timed {
            out_t.push(t);
        }
    };
    for i in 0..n {
        let t_i = if timed { times[i] } else { 0.0 };
        let Some(k) = chosen[i] else {
            push(&mut pts, &mut out_t, points[i], t_i);
            prev_matched = None;
            continue;
        };
        matched += 1;
        let here = cands[i][k];
        if let Some(p) = prev_matched {
            // the street corners passed between the previous snapped fix and this one
            if let Some(pk) = chosen[p] {
                let ca = cands[p][pk];
                let line = dist(xy[p], xy[i]);
                let spread = net.spread(&ca, (line * 3.0 + 40.0).max(80.0));
                if let Some((walk, path)) = net.route(&ca, &spread, &here) {
                    let mut at = 0.0;
                    let mut prev_xy = ca.at;
                    for node in path {
                        at += dist(prev_xy, net.nodes[node]);
                        prev_xy = net.nodes[node];
                        let f = if walk > 0.0 { (at / walk).clamp(0.0, 1.0) } else { 0.0 };
                        let t = if timed { times[p] + (times[i] - times[p]) * f } else { 0.0 };
                        push(&mut pts, &mut out_t, plane.back(net.nodes[node]), t);
                    }
                }
            }
        }
        push(&mut pts, &mut out_t, plane.back(here.at), t_i);
        prev_matched = Some(i);
    }
    // the same spot twice in a row (a standstill, or a corner that is also a fix) adds nothing
    let mut keep: Vec<bool> = vec![true; pts.len()];
    for j in 1..pts.len() {
        if dist(plane.xy(pts[j]), plane.xy(pts[j - 1])) < 0.3 {
            keep[j] = false;
        }
    }
    let pts_f: Vec<V3> = pts.iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| *p).collect();
    let t_f: Vec<f64> = if timed { out_t.iter().zip(&keep).filter(|(_, k)| **k).map(|(t, _)| *t).collect() } else { Vec::new() };
    Snapped { pts: pts_f, times: t_f, matched, total: n }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A point `east`, `north` metres from a spot in San Francisco.
    fn at(east: f64, north: f64) -> V3 {
        let (lon0, lat0) = (-122.40_f64, 37.78_f64);
        let lat = lat0 + north / 111_195.0;
        let lon = lon0 + east / (111_195.0 * lat0.to_radians().cos());
        lonlat_to_vec(lon.to_radians(), lat.to_radians())
    }

    fn metres(a: V3, b: V3) -> f64 {
        crate::clean::dist_m(a, b)
    }

    /// Distance from `p` to the nearest point of a polyline (sampled every metre).
    fn off_line(p: V3, line: &[V3]) -> f64 {
        let mut best = f64::INFINITY;
        for w in line.windows(2) {
            let len = metres(w[0], w[1]).ceil().max(1.0) as usize;
            for s in 0..=len {
                let t = s as f64 / len as f64;
                let q = crate::camera::normalize([w[0][0] + (w[1][0] - w[0][0]) * t, w[0][1] + (w[1][1] - w[0][1]) * t, w[0][2] + (w[1][2] - w[0][2]) * t]);
                best = best.min(metres(p, q));
            }
        }
        best
    }

    /// A square block of streets, 100 m a side, with a second street 40 m north of the top side.
    fn block() -> Vec<Vec<V3>> {
        vec![
            vec![at(0.0, 0.0), at(100.0, 0.0), at(100.0, 100.0), at(0.0, 100.0), at(0.0, 0.0)],
            vec![at(-50.0, 140.0), at(150.0, 140.0)],
            vec![at(0.0, 100.0), at(0.0, 140.0)],
            vec![at(100.0, 100.0), at(100.0, 140.0)],
        ]
    }

    /// A deterministic wobble in -1..1.
    fn wobble(seed: &mut u32) -> f64 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((*seed >> 8) as f64 / (1u32 << 24) as f64) * 2.0 - 1.0
    }

    /// Walks round the block (anticlockwise from the south-west corner), a fix every 8 m, with noise.
    fn noisy_square(noise: f64) -> (Vec<V3>, Vec<f64>) {
        let mut seed = 99u32;
        let corners = [(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0), (0.0, 0.0)];
        let mut pts = Vec::new();
        for w in corners.windows(2) {
            for s in 0..12 {
                let t = s as f64 / 12.0;
                let (x, y) = (w[0].0 + (w[1].0 - w[0].0) * t, w[0].1 + (w[1].1 - w[0].1) * t);
                pts.push(at(x + wobble(&mut seed) * noise, y + wobble(&mut seed) * noise));
            }
        }
        let times = (0..pts.len()).map(|i| 1000.0 + i as f64 * 6.0).collect();
        (pts, times)
    }

    #[test]
    fn a_noisy_walk_round_a_block_becomes_the_block() {
        let (pts, times) = noisy_square(12.0);
        let roads = block();
        let s = snap(&pts, &times, &roads);
        assert!(s.matched >= pts.len() * 9 / 10, "{} of {}", s.matched, s.total);
        // every output point is on the streets
        let worst = s.pts.iter().map(|p| roads.iter().map(|r| off_line(*p, r)).fold(f64::INFINITY, f64::min)).fold(0.0, f64::max);
        assert!(worst < 1.5, "{worst} m off the streets");
        // the corners were put in: the track passes through the block's three other corners
        for corner in [at(100.0, 0.0), at(100.0, 100.0), at(0.0, 100.0)] {
            let near = s.pts.iter().map(|p| metres(*p, corner)).fold(f64::INFINITY, f64::min);
            assert!(near < 1.5, "corner missed by {near} m");
        }
        // one time per point, never going backwards
        assert_eq!(s.pts.len(), s.times.len());
        assert!(s.times.windows(2).all(|w| w[1] >= w[0]));
    }

    #[test]
    fn it_does_not_jump_to_the_parallel_street() {
        // walking east along the south street; the noise sometimes pushes fixes towards the street
        // 40 m north but never past the half-way line (20 m)
        let mut seed = 7u32;
        let pts: Vec<V3> = (0..24).map(|i| at(i as f64 * 4.0, 12.0 * wobble(&mut seed).abs())).collect();
        let s = snap(&pts, &[], &block());
        assert_eq!(s.matched, pts.len());
        for p in &s.pts {
            assert!(off_line(*p, &block()[0]) < 1.0, "left the south street");
        }
    }

    #[test]
    fn fixes_far_from_any_street_stay_where_they_were() {
        // a walk across a park 200 m from the streets
        let pts: Vec<V3> = (0..10).map(|i| at(i as f64 * 5.0, -200.0)).collect();
        let s = snap(&pts, &[], &block());
        assert_eq!(s.matched, 0);
        assert_eq!(s.pts.len(), pts.len());
        assert!(s.pts.iter().zip(&pts).all(|(a, b)| metres(*a, *b) < 1e-6));
    }

    #[test]
    fn a_stretch_through_a_park_keeps_the_park_and_snaps_the_rest() {
        let mut pts: Vec<V3> = (0..8).map(|i| at(i as f64 * 6.0 + 2.0, 3.0)).collect(); // on the south street
        pts.extend((0..8).map(|i| at(60.0 + i as f64 * 6.0, -120.0))); // far south: no street
        let s = snap(&pts, &[], &block());
        assert_eq!(s.matched, 8);
        assert!(s.pts.iter().take(8).all(|p| off_line(*p, &block()[0]) < 1.0));
        assert!(s.pts.iter().skip(8).all(|p| metres(*p, at(0.0, 0.0)) > 50.0), "the park part is untouched");
    }

    #[test]
    fn no_roads_or_too_few_fixes_change_nothing() {
        let pts = vec![at(0.0, 3.0), at(10.0, 3.0)];
        assert_eq!(snap(&pts, &[], &[]).matched, 0);
        assert_eq!(snap(&pts[..1], &[], &block()).pts.len(), 1);
        let s = snap(&pts, &[5.0, 6.0], &[]);
        assert_eq!(s.times, vec![5.0, 6.0]);
    }

    #[test]
    fn route_distance_goes_round_corners() {
        let lines: Vec<Vec<[f64; 2]>> = vec![vec![[0.0, 0.0], [100.0, 0.0], [100.0, 100.0]]];
        let net = Roads::new(&lines);
        let a = net.candidates([20.0, 3.0]);
        let b = net.candidates([97.0, 70.0]);
        let (a, b) = (a[0], b[0]);
        let sp = net.spread(&a, 500.0);
        let (walk, path) = net.route(&a, &sp, &b).unwrap();
        assert!((walk - (80.0 + 70.0)).abs() < 1e-6, "{walk}");
        assert_eq!(path.len(), 1, "passes the corner junction");
    }
}
