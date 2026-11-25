//! Transit systems as data: what to ask OpenStreetMap (via Overpass) for, and how to turn the
//! answer into lines and stations. Adding another city is one more entry in `SYSTEMS`.
//! Std-only, so it is unit-tested without a GPU or network.

use std::collections::{HashMap, HashSet};

use crate::camera::{dot, lonlat_to_vec, V3};
use crate::json::{self, Json};

pub struct SystemConfig {
    pub id: &'static str,
    pub title: &'static str,
    /// south, west, north, east (degrees).
    pub bbox: [f64; 4],
    /// OSM `route=` values to include.
    pub routes: &'static str,
    /// Extra tag filters; a relation matching any one of them is included.
    pub filters: &'static [&'static str],
    /// (ref, name keyword, colour): standardised line colours, matched by ref or by name.
    pub known: &'static [(&'static str, &'static str, [u8; 3])],
    /// Where to fly to for a look.
    pub home: (f64, f64),
}

pub const SYSTEMS: [SystemConfig; 3] = [
    SystemConfig {
        id: "muni",
        title: "SF Muni Metro",
        bbox: [37.60, -122.55, 37.84, -122.35],
        routes: "subway|light_rail|tram",
        filters: &[r#"["operator"~"Muni|Municipal Railway|SFMTA"]"#, r#"["network"~"Muni|SFMTA"]"#],
        known: &[
            ("J", "Church", [247, 148, 29]),
            ("K", "Ingleside", [91, 181, 201]),
            ("L", "Taraval", [146, 39, 143]),
            ("M", "Ocean View", [0, 169, 79]),
            ("N", "Judah", [0, 114, 188]),
            ("T", "Third", [227, 28, 35]),
            ("S", "Shuttle", [255, 194, 14]),
            ("F", "Market", [160, 82, 45]),
        ],
        home: (37.7897, -122.4003),
    },
    SystemConfig {
        id: "lametro",
        title: "Los Angeles Metro",
        bbox: [33.72, -118.52, 34.20, -117.85],
        routes: "subway|light_rail",
        filters: &[r#"["network"~"LA Metro|Los Angeles Metro"]"#, r#"["operator"~"LACMTA|Los Angeles County Metropolitan|LA Metro"]"#],
        known: &[
            ("A", "A Line", [0, 114, 188]),
            ("B", "B Line", [227, 19, 27]),
            ("C", "C Line", [88, 167, 56]),
            ("D", "D Line", [160, 93, 165]),
            ("E", "E Line", [253, 185, 19]),
            ("K", "K Line", [229, 109, 177]),
            ("L", "L Line", [253, 185, 19]),
        ],
        home: (34.0562, -118.2365),
    },
    SystemConfig {
        id: "tokyometro",
        title: "Tokyo Metro",
        bbox: [35.55, 139.50, 35.86, 139.95],
        routes: "subway",
        filters: &[r#"["operator"~"Tokyo Metro|東京地下鉄|東京メトロ"]"#, r#"["network"~"Tokyo Metro|東京メトロ"]"#],
        known: &[
            ("G", "Ginza", [255, 149, 0]),
            ("M", "Marunouchi", [246, 46, 54]),
            ("m", "Marunouchi", [246, 46, 54]),
            ("H", "Hibiya", [181, 181, 172]),
            ("T", "Tozai", [0, 155, 191]),
            ("C", "Chiyoda", [0, 187, 133]),
            ("Y", "Yurakucho", [193, 164, 112]),
            ("Z", "Hanzomon", [143, 118, 214]),
            ("N", "Namboku", [0, 172, 155]),
            ("F", "Fukutoshin", [156, 94, 49]),
        ],
        home: (35.6812, 139.7671),
    },
];

pub struct TLine {
    pub name: String,
    /// Short letter/number for the end-of-line roundel.
    pub badge: String,
    pub color: [u8; 3],
    /// Unit-sphere polylines following the real track.
    pub paths: Vec<Vec<V3>>,
}

pub struct Station {
    pub name: String,
    pub pos: V3,
    /// Indices into `SystemData::lines`; two or more means an interchange.
    pub lines: Vec<usize>,
}

#[derive(Default)]
pub struct SystemData {
    pub lines: Vec<TLine>,
    pub stations: Vec<Station>,
}

/// Overpass QL for one system: route relations with full geometry, then their stop nodes.
pub fn overpass_query(c: &SystemConfig) -> String {
    let [s, w, n, e] = c.bbox;
    let mut q = String::from("[out:json][timeout:120];(");
    for f in c.filters {
        q.push_str(&format!("relation[\"route\"~\"^({})$\"]{f}({s},{w},{n},{e});", c.routes));
    }
    q.push_str(")->.r;.r out geom;(node(r.r:\"stop\");node(r.r:\"stop_entry_only\");node(r.r:\"stop_exit_only\"););out;");
    q
}

/// Percent-encodes `s` for use in a URL query value.
pub fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn parse_color(s: &str) -> Option<[u8; 3]> {
    let t = s.trim();
    if let Some(h) = t.strip_prefix('#') {
        let h = if h.len() == 3 { h.chars().flat_map(|c| [c, c]).collect::<String>() } else { h.to_string() };
        if h.len() == 6 {
            let v = u32::from_str_radix(&h, 16).ok()?;
            return Some([(v >> 16) as u8, (v >> 8) as u8, v as u8]);
        }
        return None;
    }
    Some(match t.to_ascii_lowercase().as_str() {
        "red" => [220, 40, 40],
        "orange" => [247, 148, 29],
        "yellow" => [250, 200, 20],
        "green" => [40, 160, 70],
        "blue" => [30, 100, 200],
        "purple" => [140, 70, 160],
        "pink" => [230, 110, 170],
        "brown" => [150, 90, 50],
        "gray" | "grey" => [150, 150, 150],
        "black" => [40, 40, 40],
        "teal" => [0, 150, 150],
        _ => return None,
    })
}

fn hash_color(key: &str) -> [u8; 3] {
    let mut h: u32 = 2166136261;
    for b in key.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    let hue = (h % 360) as f32 / 60.0;
    let x = 1.0 - (hue % 2.0 - 1.0).abs();
    let (r, g, b) = match hue as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    let f = |v: f32| (30.0 + v * 190.0) as u8;
    [f(r), f(g), f(b)]
}

fn is_latin(s: &str) -> bool {
    s.chars().all(|c| c.is_ascii() || ('\u{00C0}'..='\u{024F}').contains(&c) || c == '\u{2019}' || c == '\u{2013}')
}

/// English/Latin display name from an OSM `tags` object ("" if there is none).
fn latin_name(tags: &Json) -> String {
    for k in ["name:en", "name:latin", "int_name", "name"] {
        if let Some(n) = tags.str_at(k) {
            if is_latin(n) && !n.is_empty() {
                return n.to_string();
            }
        }
    }
    String::new()
}

type LonLat = (f64, f64);

fn key_of(p: LonLat) -> (i64, i64) {
    ((p.0 * 1e7).round() as i64, (p.1 * 1e7).round() as i64)
}

/// Joins ways that share end points into longer polylines (so line joins render smoothly).
pub fn chain(ways: Vec<Vec<LonLat>>) -> Vec<Vec<LonLat>> {
    let mut at: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, w) in ways.iter().enumerate() {
        if w.len() < 2 {
            continue;
        }
        at.entry(key_of(w[0])).or_default().push(i);
        at.entry(key_of(*w.last().unwrap())).or_default().push(i);
    }
    let mut used = vec![false; ways.len()];
    let mut out = Vec::new();
    for start in 0..ways.len() {
        if used[start] || ways[start].len() < 2 {
            continue;
        }
        used[start] = true;
        let mut line = ways[start].clone();
        // grow at the tail, then at the head
        for tail in [true, false] {
            loop {
                let end = if tail { *line.last().unwrap() } else { line[0] };
                let next = at.get(&key_of(end)).and_then(|v| v.iter().copied().find(|&j| !used[j]));
                let Some(j) = next else { break };
                used[j] = true;
                let mut w = ways[j].clone();
                // orient so that w begins at `end` (tail) or finishes at `end` (head)
                let begins_here = key_of(w[0]) == key_of(end);
                if tail == begins_here {
                    if tail {
                        line.extend(w.into_iter().skip(1));
                    } else {
                        w.pop();
                        w.extend(line);
                        line = w;
                    }
                } else if tail {
                    w.reverse();
                    line.extend(w.into_iter().skip(1));
                } else {
                    w.reverse();
                    w.pop();
                    w.extend(line);
                    line = w;
                }
            }
        }
        out.push(line);
    }
    out
}

fn chord_m(a: V3, b: V3) -> f64 {
    // great-circle distance in metres
    dot(a, b).clamp(-1.0, 1.0).acos() * 6_371_000.0
}

pub fn parse(cfg: &SystemConfig, text: &str) -> Result<SystemData, String> {
    let doc = json::parse(text)?;
    let elements = doc.get("elements").and_then(Json::as_arr).ok_or("no \"elements\" in response")?;

    // Stop nodes with their tags.
    let mut nodes: HashMap<i64, (LonLat, String)> = HashMap::new();
    for e in elements {
        if e.str_at("type") == Some("node") {
            if let (Some(id), Some(lat), Some(lon)) =
                (e.get("id").and_then(Json::as_f64), e.get("lat").and_then(Json::as_f64), e.get("lon").and_then(Json::as_f64))
            {
                let name = e.get("tags").map(latin_name).unwrap_or_default();
                nodes.insert(id as i64, ((lon, lat), name));
            }
        }
    }

    struct Acc {
        name: String,
        badge: String,
        color: [u8; 3],
        ways: Vec<Vec<LonLat>>,
        way_ids: HashSet<i64>,
    }
    let mut keys: HashMap<String, usize> = HashMap::new();
    let mut accs: Vec<Acc> = Vec::new();
    let mut stops: Vec<(i64, usize)> = Vec::new(); // (node id, line index)
    let mut stop_pos: HashMap<i64, LonLat> = HashMap::new();

    for e in elements {
        if e.str_at("type") != Some("relation") {
            continue;
        }
        let Some(tags) = e.get("tags") else { continue };
        let rf = tags.str_at("ref").unwrap_or("");
        let name_raw = latin_name(tags);
        let name_any = tags.str_at("name").unwrap_or("");
        let colour_tag = tags.str_at("colour").or_else(|| tags.str_at("color"));

        // Known (standardised) line?
        let known = cfg.known.iter().find(|k| {
            (!rf.is_empty() && rf == k.0) || (!k.1.is_empty() && (name_raw.contains(k.1) || name_any.contains(k.1)))
        });
        let (key, color, badge) = match known {
            Some(k) => (
                format!("known:{}", k.1),
                k.2,
                if k.0.len() <= 2 { k.0.to_uppercase() } else { String::new() },
            ),
            None => {
                let key = colour_tag
                    .map(|c| c.to_ascii_lowercase())
                    .or_else(|| (!rf.is_empty()).then(|| rf.to_string()))
                    .unwrap_or_else(|| name_any.to_string());
                let color = colour_tag.and_then(parse_color).unwrap_or_else(|| hash_color(&key));
                let badge = if !rf.is_empty() && rf.chars().count() <= 2 { rf.to_uppercase() } else { String::new() };
                (key, color, badge)
            }
        };
        let idx = *keys.entry(key).or_insert_with(|| {
            let mut name = name_raw.split(':').next().unwrap_or("").trim().to_string();
            if name.is_empty() {
                name = rf.to_string();
            }
            accs.push(Acc { name, badge, color, ways: Vec::new(), way_ids: HashSet::new() });
            accs.len() - 1
        });

        let Some(members) = e.get("members").and_then(Json::as_arr) else { continue };
        for m in members {
            let role = m.str_at("role").unwrap_or("");
            match m.str_at("type") {
                Some("way") if !role.starts_with("platform") && !role.starts_with("stop") => {
                    let id = m.get("ref").and_then(Json::as_f64).unwrap_or(-1.0) as i64;
                    if id >= 0 && !accs[idx].way_ids.insert(id) {
                        continue; // already drawn for this line (other direction / variant)
                    }
                    let pts: Vec<LonLat> = m
                        .get("geometry")
                        .and_then(Json::as_arr)
                        .map(|g| {
                            g.iter()
                                .filter_map(|p| Some((p.get("lon")?.as_f64()?, p.get("lat")?.as_f64()?)))
                                .collect()
                        })
                        .unwrap_or_default();
                    if pts.len() >= 2 {
                        accs[idx].ways.push(pts);
                    }
                }
                Some("node") if role.starts_with("stop") => {
                    if let Some(id) = m.get("ref").and_then(Json::as_f64) {
                        let id = id as i64;
                        stops.push((id, idx));
                        if let (Some(lat), Some(lon)) = (m.get("lat").and_then(Json::as_f64), m.get("lon").and_then(Json::as_f64)) {
                            stop_pos.insert(id, (lon, lat));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let to_v3 = |p: LonLat| lonlat_to_vec(p.0.to_radians(), p.1.to_radians());
    let mut data = SystemData::default();
    for a in accs {
        let paths: Vec<Vec<V3>> =
            chain(a.ways).into_iter().map(|w| w.into_iter().map(to_v3).collect()).collect();
        data.lines.push(TLine { name: a.name, badge: a.badge, color: a.color, paths });
    }

    // Stations: merge platform pairs / duplicate stops of the same name that sit close together.
    for (id, line) in stops {
        let Some(p) = nodes.get(&id).map(|n| n.0).or_else(|| stop_pos.get(&id).copied()) else { continue };
        let name = nodes.get(&id).map(|n| n.1.clone()).unwrap_or_default();
        let pos = to_v3(p);
        let radius = if name.is_empty() { 40.0 } else { 300.0 };
        let hit = data
            .stations
            .iter_mut()
            .find(|s| s.name == name && chord_m(s.pos, pos) < radius);
        match hit {
            Some(s) => {
                if !s.lines.contains(&line) {
                    s.lines.push(line);
                }
            }
            None => data.stations.push(Station { name, pos, lines: vec![line] }),
        }
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_joins_ways_in_any_orientation() {
        let ways = vec![
            vec![(0.0, 0.0), (1.0, 0.0)],
            vec![(2.0, 0.0), (1.0, 0.0)], // reversed
            vec![(2.0, 0.0), (3.0, 1.0)],
            vec![(10.0, 10.0), (11.0, 10.0)], // separate piece
            vec![(-1.0, 0.0), (0.0, 0.0)],    // joins at the head
        ];
        let mut out = chain(ways);
        out.sort_by_key(|l| l.len());
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].len(), 2);
        let long = &out[1];
        assert_eq!(long.len(), 5);
        let xs: Vec<f64> = long.iter().map(|p| p.0).collect();
        assert!(xs == [-1.0, 0.0, 1.0, 2.0, 3.0] || xs == [3.0, 2.0, 1.0, 0.0, -1.0], "{xs:?}");
    }

    #[test]
    fn query_mentions_bbox_and_filters() {
        let q = overpass_query(&SYSTEMS[2]);
        assert!(q.contains("35.55,139.5,35.86,139.95"), "{q}");
        assert!(q.contains("Tokyo Metro"));
        assert!(q.contains("out geom"));
        let e = url_encode(&q);
        assert!(!e.contains(' ') && !e.contains('"') && e.contains("%5B"));
    }

    const SAMPLE: &str = r##"{"elements":[
      {"type":"relation","id":1,"tags":{"route":"subway","ref":"G","name":"Tokyo Metro Ginza Line: Shibuya => Asakusa","colour":"#ff9500"},
       "members":[
         {"type":"way","ref":100,"role":"","geometry":[{"lat":35.0,"lon":139.0},{"lat":35.0,"lon":139.1}]},
         {"type":"way","ref":101,"role":"","geometry":[{"lat":35.0,"lon":139.1},{"lat":35.1,"lon":139.1}]},
         {"type":"node","ref":7,"role":"stop","lat":35.0,"lon":139.0},
         {"type":"node","ref":8,"role":"stop","lat":35.1,"lon":139.1}]},
      {"type":"relation","id":2,"tags":{"route":"subway","ref":"G","name":"Tokyo Metro Ginza Line: Asakusa => Shibuya"},
       "members":[
         {"type":"way","ref":101,"role":"","geometry":[{"lat":35.0,"lon":139.1},{"lat":35.1,"lon":139.1}]},
         {"type":"node","ref":9,"role":"stop","lat":35.0001,"lon":139.0001}]},
      {"type":"relation","id":3,"tags":{"route":"subway","ref":"X","name":"Mystery","colour":"purple"},
       "members":[{"type":"way","ref":300,"role":"","geometry":[{"lat":35.0,"lon":139.0},{"lat":35.0,"lon":139.05}]},
                  {"type":"node","ref":7,"role":"stop","lat":35.0,"lon":139.0}]},
      {"type":"node","id":7,"lat":35.0,"lon":139.0,"tags":{"name":"渋谷","name:en":"Shibuya"}},
      {"type":"node","id":8,"lat":35.1,"lon":139.1,"tags":{"name":"浅草"}},
      {"type":"node","id":9,"lat":35.0001,"lon":139.0001,"tags":{"name:en":"Shibuya"}}
    ]}"##;

    #[test]
    fn parses_lines_dedupes_ways_and_merges_stations() {
        let d = parse(&SYSTEMS[2], SAMPLE).unwrap();
        assert_eq!(d.lines.len(), 2, "Ginza (both directions merged) + Mystery");
        let ginza = &d.lines[0];
        assert_eq!(ginza.color, [255, 149, 0], "standardised colour wins");
        assert_eq!(ginza.badge, "G");
        assert_eq!(ginza.paths.len(), 1, "two ways chained into one polyline");
        assert_eq!(ginza.paths[0].len(), 3);
        assert_eq!(d.lines[1].color, [140, 70, 160]);
        // Shibuya: nodes 7 and 9 merged, used by Ginza and Mystery => interchange
        let shibuya = d.stations.iter().find(|s| s.name == "Shibuya").unwrap();
        assert_eq!(shibuya.lines.len(), 2);
        assert_eq!(d.stations.iter().filter(|s| s.name == "Shibuya").count(), 1);
        // Asakusa has only a Japanese name: kept as an unnamed dot
        assert!(d.stations.iter().any(|s| s.name.is_empty() && s.lines == vec![0]));
    }
}
