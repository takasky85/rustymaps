//! Reads GPS tracks from KML files (what phone trackers save): `<Placemark>`s holding a
//! `<LineString><coordinates>` or a Google `<gx:Track>` with `<gx:coord>` points. Plain text
//! scanning, no XML library: a damaged or unknown file just yields fewer (or no) tracks.

use crate::camera::{dot, lonlat_to_vec, V3};

/// One track: a name and its stretches (a walk recorded with pauses is often several).
pub struct Parsed {
    pub name: String,
    pub segs: Vec<Vec<V3>>,
}

/// Points closer than this to the previous kept one are dropped (GPS jitter while standing still).
const MIN_STEP_M: f64 = 2.0;
const EARTH_M: f64 = 6_371_000.0;

/// The start of the next `<tag ...>` at or after `from`: (index after the `>`, self-closing).
fn open_tag(s: &str, tag: &str, from: usize) -> Option<(usize, bool)> {
    let pat = format!("<{tag}");
    let mut at = from;
    while let Some(k) = s.get(at..)?.find(&pat) {
        let start = at + k;
        let after = start + pat.len();
        match s.as_bytes().get(after) {
            Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') | Some(b'/') => {
                let gt = s.get(after..)?.find('>')? + after;
                return Some((gt + 1, s.as_bytes()[gt - 1] == b'/'));
            }
            _ => at = after,
        }
    }
    None
}

/// The text inside the next `<tag>...</tag>` at or after `from`, and the index after it.
fn element<'a>(s: &'a str, tag: &str, from: usize) -> Option<(&'a str, usize)> {
    let mut at = from;
    loop {
        let (inner, selfclosing) = open_tag(s, tag, at)?;
        if selfclosing {
            at = inner;
            continue;
        }
        let close = format!("</{tag}>");
        let end = s.get(inner..)?.find(&close)? + inner;
        return Some((&s[inner..end], end + close.len()));
    }
}

fn unescape(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix("<![CDATA[").and_then(|t| t.strip_suffix("]]>")).unwrap_or(s);
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&").trim().to_string()
}

fn point(lon: f64, lat: f64) -> Option<V3> {
    (lon.is_finite() && lat.is_finite() && lon.abs() <= 180.0 && lat.abs() <= 90.0).then(|| lonlat_to_vec(lon.to_radians(), lat.to_radians()))
}

/// Keeps a point only when it is some metres from the last one kept.
fn thin(points: Vec<V3>) -> Vec<V3> {
    let min = MIN_STEP_M / EARTH_M;
    let min_chord2 = min * min;
    let mut out: Vec<V3> = Vec::with_capacity(points.len());
    for p in points {
        match out.last() {
            Some(&l) if 2.0 - 2.0 * dot(l, p) < min_chord2 => {}
            _ => out.push(p),
        }
    }
    out
}

fn block(text: &str, fallback: &str) -> Option<Parsed> {
    let name = element(text, "name", 0).map(|(n, _)| unescape(n)).filter(|n| !n.is_empty()).unwrap_or_else(|| fallback.to_string());
    let mut segs: Vec<Vec<V3>> = Vec::new();
    // <coordinates>lon,lat[,alt] lon,lat[,alt] ...</coordinates>
    let mut at = 0;
    while let Some((inner, next)) = element(text, "coordinates", at) {
        at = next;
        let pts: Vec<V3> = inner
            .split_whitespace()
            .filter_map(|t| {
                let mut it = t.split(',');
                let lon = it.next()?.parse::<f64>().ok()?;
                let lat = it.next()?.parse::<f64>().ok()?;
                point(lon, lat)
            })
            .collect();
        segs.push(thin(pts));
    }
    // <gx:Track> ... <gx:coord>lon lat alt</gx:coord> ... </gx:Track>
    let mut at = 0;
    while let Some((track, next)) = element(text, "gx:Track", at) {
        at = next;
        let mut pts: Vec<V3> = Vec::new();
        let mut c = 0;
        while let Some((inner, n2)) = element(track, "gx:coord", c) {
            c = n2;
            let mut it = inner.split_whitespace();
            if let (Some(lon), Some(lat)) = (it.next().and_then(|v| v.parse::<f64>().ok()), it.next().and_then(|v| v.parse::<f64>().ok())) {
                pts.extend(point(lon, lat));
            }
        }
        segs.push(thin(pts));
    }
    // a single point is a waypoint, not a track
    segs.retain(|s| s.len() >= 2);
    (!segs.is_empty()).then_some(Parsed { name, segs })
}

/// Every track in a KML document. `fallback` names tracks that have no name of their own.
pub fn parse(text: &str, fallback: &str) -> Vec<Parsed> {
    let mut out = Vec::new();
    let mut at = 0;
    let mut any = false;
    while let Some((inner, next)) = element(text, "Placemark", at) {
        any = true;
        at = next;
        out.extend(block(inner, fallback));
    }
    if !any {
        out.extend(block(text, fallback));
    }
    out
}

/// Length of a stretch along the ground, in kilometres.
pub fn length_km(seg: &[V3]) -> f64 {
    seg.windows(2).map(|w| dot(w[0], w[1]).clamp(-1.0, 1.0).acos()).sum::<f64>() * EARTH_M / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<kml xmlns="http://www.opengis.net/kml/2.2" xmlns:gx="http://www.google.com/kml/ext/2.2">
<Document>
  <name>My phone</name>
  <Placemark>
    <name><![CDATA[Belgum Trail &amp; loop]]></name>
    <LineString><tessellate>1</tessellate>
      <coordinates>
        -122.3000,37.9000,10 -122.3000,37.9000,10 -122.2990,37.9010,11
        -122.2980,37.9020,12 junk -122.2970,37.9030
      </coordinates>
    </LineString>
  </Placemark>
  <Placemark id="w">
    <name>Parking</name>
    <Point><coordinates>-122.3,37.9,0</coordinates></Point>
  </Placemark>
  <Placemark>
    <gx:Track>
      <when>2026-11-28T12:00:00Z</when><gx:coord>139.70 35.68 5</gx:coord>
      <when>2026-11-28T12:00:05Z</when><gx:coord>139.71 35.69 5</gx:coord>
      <when>2026-11-28T12:00:10Z</when><gx:coord>999 35.70 5</gx:coord>
    </gx:Track>
  </Placemark>
</Document></kml>"#;

    #[test]
    fn reads_linestrings_and_gx_tracks_and_skips_waypoints() {
        let t = parse(DOC, "file");
        assert_eq!(t.len(), 2, "the waypoint is not a track");
        assert_eq!(t[0].name, "Belgum Trail & loop");
        assert_eq!(t[0].segs.len(), 1);
        // the repeated point and the junk token are gone: 4 points remain
        assert_eq!(t[0].segs[0].len(), 4);
        let km = length_km(&t[0].segs[0]);
        assert!(km > 0.3 && km < 0.5, "{km}");
        // second one has no name: falls back; the out-of-range longitude is skipped
        assert_eq!(t[1].name, "file");
        assert_eq!(t[1].segs[0].len(), 2);
    }

    #[test]
    fn bare_coordinates_without_placemarks_still_work() {
        let t = parse("<coordinates>1,2,0 1.01,2.01,0</coordinates>", "bare");
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].name, "bare");
        assert!(parse("not xml at all", "x").is_empty());
        assert!(parse("", "x").is_empty());
    }

    #[test]
    fn jitter_is_thinned() {
        let mut s = String::from("<coordinates>");
        for i in 0..1000 {
            // 1000 points within ~1 m of each other, then a real move
            s.push_str(&format!("10.0{:05},50.0 ", i % 3));
        }
        s.push_str("10.01,50.0</coordinates>");
        let t = parse(&s, "j");
        assert_eq!(t.len(), 1);
        assert!(t[0].segs[0].len() < 10, "{}", t[0].segs[0].len());
    }
}
