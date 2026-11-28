//! Finds the streets around a GPS track in the map tiles already on disk, and snaps the track to
//! them (see `snap`). Nothing is downloaded: tiles that were never viewed are simply counted as
//! missing, and those stretches stay as they were.

use crate::camera::{lonlat_to_vec, vec_to_lonlat, V3};
use crate::mvt;
use crate::replay::Timeline;
use crate::snap;
use crate::tile_id::TileId;
use crate::tiles::{DiskCache, MAX_TILE_ZOOM};

/// A track with its stretches snapped to the streets.
pub struct SnappedTrack {
    pub segs: Vec<Vec<V3>>,
    pub timeline: Timeline,
    pub matched: usize,
    pub total: usize,
    /// Map tiles that cover the track but are not on disk (so those parts could not be snapped).
    pub missing_tiles: usize,
}

/// Most tiles looked at for one track; a track bigger than this is a journey, not a walk.
const MAX_TILES: usize = 100;
/// Road classes a person can be on (everything except rails and ferries).
fn is_street(class: &str) -> bool {
    !matches!(class, "rail" | "transit" | "ferry" | "aerialway" | "")
}

fn tile_xy(lon: f64, lat: f64) -> (f64, f64) {
    let n = (1u64 << MAX_TILE_ZOOM) as f64;
    let x = (lon + std::f64::consts::PI) / (2.0 * std::f64::consts::PI) * n;
    let y = (1.0 - lat.tan().asinh() / std::f64::consts::PI) * 0.5 * n;
    (x, y)
}

/// Street polylines (as points on the globe) from one tile's bytes.
fn streets_in_tile(id: TileId, bytes: &[u8]) -> Vec<Vec<V3>> {
    let mut out = Vec::new();
    let Ok(layers) = mvt::decode_tile(bytes) else { return out };
    let n = (1u64 << id.z) as f64;
    for layer in layers.iter().filter(|l| l.name == "transportation") {
        let extent = layer.extent.max(1) as f64;
        for f in &layer.features {
            if f.geom_type != 2 || !is_street(layer.str_prop(f, "class").unwrap_or("")) {
                continue;
            }
            for path in f.paths() {
                let line: Vec<V3> = path
                    .iter()
                    .map(|&(px, py)| {
                        let mx = (id.x as f64 + px as f64 / extent) / n;
                        let my = (id.y as f64 + py as f64 / extent) / n;
                        let lon = (mx - 0.5) * 2.0 * std::f64::consts::PI;
                        let lat = (std::f64::consts::PI * (1.0 - 2.0 * my)).sinh().atan();
                        lonlat_to_vec(lon, lat)
                    })
                    .collect();
                if line.len() >= 2 {
                    out.push(line);
                }
            }
        }
    }
    out
}

/// The streets near `points` from the tiles on disk, and how many of the tiles needed are missing.
fn streets_near(points: &[V3], cache: &DiskCache) -> (Vec<Vec<V3>>, usize) {
    let (mut lo_x, mut hi_x, mut lo_y, mut hi_y) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
    for &p in points {
        let (lon, lat) = vec_to_lonlat(p);
        let (x, y) = tile_xy(lon, lat);
        lo_x = lo_x.min(x);
        hi_x = hi_x.max(x);
        lo_y = lo_y.min(y);
        hi_y = hi_y.max(y);
    }
    if !lo_x.is_finite() {
        return (Vec::new(), 0);
    }
    // a margin of about 100 m (a tile is ~2.4 km wide at zoom 14 in San Francisco)
    let pad = 0.05;
    let (x0, x1, y0, y1) = ((lo_x - pad).floor() as i64, (hi_x + pad).floor() as i64, (lo_y - pad).floor() as i64, (hi_y + pad).floor() as i64);
    let count = ((x1 - x0 + 1) * (y1 - y0 + 1)) as usize;
    if count > MAX_TILES {
        return (Vec::new(), 0);
    }
    let mut lines = Vec::new();
    let mut missing = 0;
    for x in x0..=x1 {
        for y in y0..=y1 {
            let id = TileId { z: MAX_TILE_ZOOM, x: x.max(0) as u32, y: y.max(0) as u32 };
            match cache.get_any(id) {
                Some(bytes) if !bytes.is_empty() => lines.extend(streets_in_tile(id, &bytes)),
                Some(_) => {} // known to be empty (open sea, say)
                None => missing += 1,
            }
        }
    }
    (lines, missing)
}

/// Snaps the tidied stretches of a track. `clean_times` is empty or one list per stretch.
pub fn snap_track(clean_segs: &[Vec<V3>], clean_times: &[Vec<f64>], span: Option<f64>, cache: &DiskCache) -> SnappedTrack {
    let all: Vec<V3> = clean_segs.iter().flatten().copied().collect();
    let (roads, missing_tiles) = streets_near(&all, cache);
    let mut segs = Vec::new();
    let mut times = Vec::new();
    let (mut matched, mut total) = (0, 0);
    for (i, s) in clean_segs.iter().enumerate() {
        let t: &[f64] = clean_times.get(i).map_or(&[], |t| &t[..]);
        let r = snap::snap(s, t, &roads);
        matched += r.matched;
        total += r.total;
        segs.push(r.pts);
        times.push(r.times);
    }
    let timeline = Timeline::new(&segs, &times, span);
    SnappedTrack { segs, timeline, matched, total, missing_tiles }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvt::test_enc::*;

    /// A tile with one street running east through the middle, and a railway along the top.
    fn tile_with_street() -> Vec<u8> {
        tile(
            "transportation",
            vec![
                Feat { geom: Geom::Line(vec![(0, 2048), (4096, 2048)]), props: vec![("class", Val::S("minor"))] },
                Feat { geom: Geom::Line(vec![(0, 100), (4096, 100)]), props: vec![("class", Val::S("rail"))] },
            ],
        )
    }

    #[test]
    fn streets_come_from_a_tile_and_rails_are_left_out() {
        let id = TileId { z: 14, x: 2621, y: 6333 };
        let lines = streets_in_tile(id, &tile_with_street());
        assert_eq!(lines.len(), 1, "the railway is not a street");
        let (lon0, lat0) = vec_to_lonlat(lines[0][0]);
        let (lon1, lat1) = vec_to_lonlat(lines[0][1]);
        assert!((lat0 - lat1).abs() < 1e-9 && lon1 > lon0, "runs east");
        // the street is inside the tile it came from
        // (the middle of the street, so rounding at the tile's edge cannot matter)
        let (tx, ty) = tile_xy((lon0 + lon1) * 0.5, (lat0 + lat1) * 0.5);
        assert!((tx.floor() as u32, ty.floor() as u32) == (id.x, id.y), "{tx} {ty}");
    }

    #[test]
    fn tile_numbers_match_the_usual_scheme() {
        // San Francisco City Hall is in tile 14/2620/6332 (zoom 14 of the OSM scheme)
        let (x, y) = tile_xy((-122.4192_f64).to_radians(), (37.7793_f64).to_radians());
        assert_eq!((x.floor() as u32, y.floor() as u32), (2620, 6332));
    }
}
