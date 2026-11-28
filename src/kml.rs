//! Reads GPS tracks from KML files (what phone trackers save): `<Placemark>`s holding a
//! `<LineString><coordinates>` or a Google `<gx:Track>` with `<gx:coord>` points. Plain text
//! scanning, no XML library: a damaged or unknown file just yields fewer (or no) tracks.

use crate::camera::{dot, lonlat_to_vec, V3};

/// One track: a name and its stretches (a walk recorded with pauses is often several).
pub struct Parsed {
    pub name: String,
    pub segs: Vec<Vec<V3>>,
    /// For each stretch, the time of every point in seconds since 1970 (UTC); empty when the file
    /// gave no times for that stretch.
    pub times: Vec<Vec<f64>>,
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

/// Keeps a point only when it is some metres from the last one kept (its time, if any, goes with it).
fn thin(points: Vec<V3>, times: Vec<f64>) -> (Vec<V3>, Vec<f64>) {
    let min = MIN_STEP_M / EARTH_M;
    let min_chord2 = min * min;
    let timed = times.len() == points.len();
    let mut out: Vec<V3> = Vec::with_capacity(points.len());
    let mut out_t: Vec<f64> = Vec::new();
    for (i, p) in points.into_iter().enumerate() {
        match out.last() {
            Some(&l) if 2.0 - 2.0 * dot(l, p) < min_chord2 => {}
            _ => {
                out.push(p);
                if timed {
                    out_t.push(times[i]);
                }
            }
        }
    }
    (out, out_t)
}

/// Days from 1970-01-01 to a calendar date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// "2026-11-28T12:00:05Z" (also with fractions of a second, a space instead of T, or a +hh:mm
/// offset) as seconds since 1970 UTC.
pub fn parse_time(text: &str) -> Option<f64> {
    let t = text.trim();
    if !t.is_ascii() || t.len() < 19 {
        return None;
    }
    let num = |a: usize, b: usize| t.get(a..b)?.parse::<i64>().ok();
    let b = t.as_bytes();
    if b[4] != b'-' || b[7] != b'-' || (b[10] != b'T' && b[10] != b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (y, mo, d, h, mi, sec) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut rest = &t[19..];
    let mut frac = 0.0;
    if let Some(r) = rest.strip_prefix('.') {
        let n = r.bytes().take_while(|c| c.is_ascii_digit()).count();
        frac = format!("0.{}", &r[..n]).parse::<f64>().unwrap_or(0.0);
        rest = &r[n..];
    }
    let mut offset = 0i64;
    if rest.starts_with('+') || rest.starts_with('-') {
        let sign = if rest.starts_with('-') { -1 } else { 1 };
        let digits: String = rest[1..].chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() >= 2 {
            let oh = digits[..2].parse::<i64>().ok()?;
            let om = if digits.len() >= 4 { digits[2..4].parse::<i64>().ok()? } else { 0 };
            offset = sign * (oh * 3600 + om * 60);
        }
    }
    Some((days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec - offset) as f64 + frac)
}

/// How long a track lasted, from a name like "Track 2 14:05-14:32" (what the phone tracker writes):
/// 27 minutes in seconds. Wraps past midnight; `None` when the name has no such range.
pub fn name_span_secs(name: &str) -> Option<f64> {
    let b = name.as_bytes();
    let d = |i: usize| b.get(i).map_or(false, |c| c.is_ascii_digit());
    let n = |i: usize| ((b[i] - b'0') * 10 + (b[i + 1] - b'0')) as i64;
    for i in 0..b.len().saturating_sub(10) {
        if d(i) && d(i + 1) && b[i + 2] == b':' && d(i + 3) && d(i + 4) && b[i + 5] == b'-' && d(i + 6) && d(i + 7) && b[i + 8] == b':' && d(i + 9) && d(i + 10) {
            let (a, z) = (n(i) * 60 + n(i + 3), n(i + 6) * 60 + n(i + 9));
            if n(i) > 23 || n(i + 6) > 23 || n(i + 3) > 59 || n(i + 9) > 59 {
                continue;
            }
            let mins = if z >= a { z - a } else { z + 1440 - a };
            return (mins > 0).then_some(mins as f64 * 60.0);
        }
    }
    None
}

fn block(text: &str, fallback: &str) -> Option<Parsed> {
    let name = element(text, "name", 0).map(|(n, _)| unescape(n)).filter(|n| !n.is_empty()).unwrap_or_else(|| fallback.to_string());
    let mut stretches: Vec<(Vec<V3>, Vec<f64>)> = Vec::new();
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
        stretches.push(thin(pts, Vec::new()));
    }
    // <gx:Track> ... <when>time</when> ... <gx:coord>lon lat alt</gx:coord> ... </gx:Track>
    let mut at = 0;
    while let Some((track, next)) = element(text, "gx:Track", at) {
        at = next;
        let mut whens: Vec<Option<f64>> = Vec::new();
        let mut w = 0;
        while let Some((inner, n2)) = element(track, "when", w) {
            w = n2;
            whens.push(parse_time(inner));
        }
        let mut coords: Vec<Option<V3>> = Vec::new();
        let mut c = 0;
        while let Some((inner, n2)) = element(track, "gx:coord", c) {
            c = n2;
            let mut it = inner.split_whitespace();
            let lon = it.next().and_then(|v| v.parse::<f64>().ok());
            let lat = it.next().and_then(|v| v.parse::<f64>().ok());
            coords.push(lon.zip(lat).and_then(|(lon, lat)| point(lon, lat)));
        }
        // times belong to coordinates one for one; if the counts differ the times are not trusted
        let timed = whens.len() == coords.len() && whens.iter().all(|t| t.is_some());
        let mut pts: Vec<V3> = Vec::new();
        let mut times: Vec<f64> = Vec::new();
        for (i, p) in coords.into_iter().enumerate() {
            if let Some(p) = p {
                pts.push(p);
                if timed {
                    times.push(whens[i].unwrap_or(0.0));
                }
            }
        }
        // a clock that runs backwards cannot be a track's times
        if times.windows(2).any(|w| w[1] < w[0]) {
            times.clear();
        }
        stretches.push(thin(pts, times));
    }
    // a single point is a waypoint, not a track
    stretches.retain(|s| s.0.len() >= 2);
    if stretches.is_empty() {
        return None;
    }
    let (segs, times): (Vec<_>, Vec<_>) = stretches.into_iter().unzip();
    Some(Parsed { name, segs, times })
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
    fn times_come_with_their_points() {
        let t = parse(DOC, "file");
        assert!(t[0].times[0].is_empty(), "a LineString has no times");
        // the bad longitude's time is dropped together with its point
        assert_eq!(t[1].times[0].len(), t[1].segs[0].len());
        assert_eq!(t[1].times[0], vec![1_795_867_200.0, 1_795_867_205.0]);
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(parse_time("2000-03-01T00:00:00Z"), Some(951_868_800.0));
        assert_eq!(parse_time("2026-11-28T05:00:00-07:00"), parse_time("2026-11-28T12:00:00Z"));
        assert_eq!(parse_time("2026-11-28T12:00:00.5Z"), Some(1_795_867_200.5));
        assert_eq!(parse_time("yesterday"), None);
    }

    #[test]
    fn a_track_name_with_a_time_range_gives_its_duration() {
        assert_eq!(name_span_secs("Track 1 14:05-14:32"), Some(27.0 * 60.0));
        assert_eq!(name_span_secs("Track 2 23:50-00:10"), Some(20.0 * 60.0));
        assert_eq!(name_span_secs("Belgum Trail"), None);
        assert_eq!(name_span_secs("Track 3 09:00-09:00"), None);
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
