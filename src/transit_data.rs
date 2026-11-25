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


const fn sys(
    id: &'static str,
    title: &'static str,
    bbox: [f64; 4],
    routes: &'static str,
    filters: &'static [&'static str],
    known: &'static [(&'static str, &'static str, [u8; 3])],
    home: (f64, f64),
) -> SystemConfig {
    SystemConfig { id, title, bbox, routes, filters, known, home }
}

/// No extra filter: every relation of the wanted `route=` kinds inside the box.
const ANY: &[&str] = &[""];

pub const SYSTEMS: &[SystemConfig] = &[
    sys(
        "muni",
        "SF Muni Metro",
        [37.60, -122.55, 37.84, -122.35],
        "subway|light_rail|tram",
        &[r#"["operator"~"Muni|Municipal Railway|SFMTA"]"#, r#"["network"~"Muni|SFMTA"]"#],
        &[
            ("J", "Church", [247, 148, 29]),
            ("K", "Ingleside", [91, 181, 201]),
            ("L", "Taraval", [146, 39, 143]),
            ("M", "Ocean View", [0, 169, 79]),
            ("N", "Judah", [0, 114, 188]),
            ("T", "Third", [227, 28, 35]),
            ("S", "Shuttle", [255, 194, 14]),
            ("F", "Market", [160, 82, 45]),
        ],
        (37.7897, -122.4003),
    ),
    sys(
        "lametro",
        "Los Angeles Metro",
        [33.72, -118.52, 34.20, -117.85],
        "subway|light_rail",
        &[r#"["network"~"LA Metro|Los Angeles Metro"]"#, r#"["operator"~"LACMTA|Los Angeles County Metropolitan|LA Metro"]"#],
        &[
            ("A", "A Line", [0, 114, 188]),
            ("B", "B Line", [227, 19, 27]),
            ("C", "C Line", [88, 167, 56]),
            ("D", "D Line", [160, 93, 165]),
            ("E", "E Line", [253, 185, 19]),
            ("K", "K Line", [229, 109, 177]),
            ("L", "L Line", [253, 185, 19]),
        ],
        (34.0562, -118.2365),
    ),
    sys(
        "tokyometro",
        "Tokyo Metro",
        [35.55, 139.50, 35.86, 139.95],
        "subway",
        &[r#"["operator"~"Tokyo Metro|東京地下鉄|東京メトロ"]"#, r#"["network"~"Tokyo Metro|東京メトロ"]"#],
        &[
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
        (35.6812, 139.7671),
    ),
    sys(
        "toei",
        "Tokyo Toei Subway",
        [35.55, 139.50, 35.86, 139.95],
        "subway",
        &[r#"["operator"~"Toei|東京都交通局"]"#, r#"["network"~"Toei|都営"]"#],
        &[("A", "Asakusa", [232, 82, 152]), ("I", "Mita", [0, 121, 194]), ("S", "Shinjuku", [108, 187, 90]), ("E", "Oedo", [182, 0, 122])],
        (35.6896, 139.7006),
    ),
    sys(
        "london",
        "London Underground & DLR",
        [51.28, -0.65, 51.75, 0.30],
        "subway|light_rail",
        &[r#"["network"~"London Underground"]"#, r#"["network"~"Docklands Light Railway"]"#],
        &[
            ("", "bakerloo", [178, 99, 0]),
            ("", "central line", [220, 36, 31]),
            ("", "circle line", [255, 200, 10]),
            ("", "district line", [0, 125, 50]),
            ("", "hammersmith", [244, 169, 190]),
            ("", "jubilee", [134, 143, 152]),
            ("", "metropolitan", [155, 0, 88]),
            ("", "northern line", [20, 20, 20]),
            ("", "piccadilly", [0, 25, 168]),
            ("", "victoria line", [0, 152, 216]),
            ("", "waterloo & city", [147, 206, 186]),
            ("", "docklands", [0, 175, 170]),
        ],
        (51.5074, -0.1278),
    ),
    sys(
        "newyork",
        "New York City Subway",
        [40.50, -74.28, 40.92, -73.70],
        "subway",
        ANY,
        &[
            ("1", "", [238, 53, 46]),
            ("2", "", [238, 53, 46]),
            ("3", "", [238, 53, 46]),
            ("4", "", [0, 147, 60]),
            ("5", "", [0, 147, 60]),
            ("6", "", [0, 147, 60]),
            ("7", "", [185, 51, 173]),
            ("A", "", [0, 57, 166]),
            ("C", "", [0, 57, 166]),
            ("E", "", [0, 57, 166]),
            ("B", "", [255, 99, 25]),
            ("D", "", [255, 99, 25]),
            ("F", "", [255, 99, 25]),
            ("M", "", [255, 99, 25]),
            ("G", "", [108, 190, 69]),
            ("J", "", [153, 102, 51]),
            ("Z", "", [153, 102, 51]),
            ("L", "", [167, 169, 172]),
            ("N", "", [252, 204, 10]),
            ("Q", "", [252, 204, 10]),
            ("R", "", [252, 204, 10]),
            ("W", "", [252, 204, 10]),
            ("S", "", [128, 129, 131]),
        ],
        (40.7527, -73.9772),
    ),
    sys(
        "paris",
        "Paris Métro",
        [48.70, 2.10, 49.00, 2.60],
        "subway",
        ANY,
        &[
            ("1", "", [255, 205, 0]),
            ("2", "", [0, 60, 166]),
            ("3", "", [131, 121, 2]),
            ("3bis", "", [110, 196, 232]),
            ("4", "", [207, 0, 158]),
            ("5", "", [255, 126, 46]),
            ("6", "", [110, 202, 151]),
            ("7", "", [250, 154, 186]),
            ("7bis", "", [110, 202, 151]),
            ("8", "", [225, 155, 223]),
            ("9", "", [182, 189, 0]),
            ("10", "", [201, 145, 13]),
            ("11", "", [112, 75, 28]),
            ("12", "", [0, 120, 82]),
            ("13", "", [110, 196, 232]),
            ("14", "", [98, 37, 157]),
        ],
        (48.8566, 2.3522),
    ),
    sys("moscow", "Moscow Metro", [55.55, 37.30, 55.95, 37.95], "subway|light_rail", ANY, &[], (55.7558, 37.6173)),
    sys("beijing", "Beijing Subway", [39.70, 116.05, 40.25, 116.75], "subway", ANY, &[], (39.9087, 116.3975)),
    sys("shanghai", "Shanghai Metro", [30.90, 121.20, 31.45, 121.85], "subway", ANY, &[], (31.2304, 121.4737)),
    sys("guangzhou", "Guangzhou Metro", [22.95, 113.10, 23.35, 113.55], "subway", ANY, &[], (23.1291, 113.2644)),
    sys("shenzhen", "Shenzhen Metro", [22.45, 113.80, 22.80, 114.35], "subway", ANY, &[], (22.5431, 114.0579)),
    sys("chengdu", "Chengdu Metro", [30.45, 103.85, 30.85, 104.30], "subway", ANY, &[], (30.5728, 104.0668)),
    sys("wuhan", "Wuhan Metro", [30.35, 114.05, 30.75, 114.55], "subway", ANY, &[], (30.5928, 114.3055)),
    sys("nanjing", "Nanjing Metro", [31.85, 118.50, 32.25, 119.05], "subway", ANY, &[], (32.0603, 118.7969)),
    sys("hangzhou", "Hangzhou Metro", [30.10, 120.00, 30.40, 120.45], "subway", ANY, &[], (30.2741, 120.1551)),
    sys("osaka", "Osaka Metro", [34.45, 135.30, 34.85, 135.70], "subway", ANY, &[], (34.7025, 135.4959)),
    sys("seoul", "Seoul Metropolitan Subway", [37.35, 126.70, 37.75, 127.30], "subway", ANY, &[], (37.5547, 126.9707)),
    sys("delhi", "Delhi Metro", [28.35, 76.85, 28.90, 77.50], "subway", ANY, &[], (28.6328, 77.2197)),
    sys("hongkong", "Hong Kong MTR", [22.20, 113.90, 22.45, 114.30], "subway", ANY, &[], (22.2819, 114.1583)),
    sys("singapore", "Singapore MRT & LRT", [1.22, 103.60, 1.48, 104.05], "subway|light_rail", ANY, &[], (1.2830, 103.8513)),
    sys("mexicocity", "Mexico City Metro", [19.30, -99.30, 19.55, -98.95], "subway", ANY, &[], (19.4326, -99.1332)),
    sys("madrid", "Madrid Metro", [40.25, -3.90, 40.60, -3.45], "subway|light_rail", ANY, &[], (40.4168, -3.7038)),
    sys("berlin", "Berlin U-Bahn", [52.35, 13.05, 52.65, 13.80], "subway", ANY, &[], (52.5200, 13.4050)),
    sys("cairo", "Cairo Metro", [29.95, 31.15, 30.20, 31.50], "subway", ANY, &[], (30.0444, 31.2357)),
    sys("saopaulo", "São Paulo Metro", [-23.75, -46.85, -23.40, -46.45], "subway", ANY, &[], (-23.5505, -46.6333)),
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
    /// Unit vector at the middle of everything, and the angular radius (radians) that contains it.
    pub center: V3,
    pub radius: f64,
}

impl SystemData {
    /// Computes `center` and `radius`; call after the lines are filled in.
    pub fn finish(&mut self) {
        let mut sum = [0.0f64; 3];
        let mut n = 0usize;
        for p in self.lines.iter().flat_map(|l| l.paths.iter()).flat_map(|p| p.iter().step_by(4)) {
            sum = [sum[0] + p[0], sum[1] + p[1], sum[2] + p[2]];
            n += 1;
        }
        if n == 0 {
            self.center = [0.0, 0.0, 1.0];
            self.radius = 0.0;
            return;
        }
        let c = crate::camera::normalize(sum);
        let mut worst = 0.0f64;
        for p in self.lines.iter().flat_map(|l| l.paths.iter()).flat_map(|p| p.iter()) {
            worst = worst.max(dot(c, *p).clamp(-1.0, 1.0).acos());
        }
        self.center = c;
        self.radius = worst;
    }

    pub fn point_count(&self) -> usize {
        self.lines.iter().flat_map(|l| l.paths.iter()).map(|p| p.len()).sum()
    }
}

// ---------------------------------------------------------------------------
// Compact on-disk format (`<id>.tmap`). The raw Overpass answer is tens of megabytes per big city;
// this keeps only what is drawn, at about 1 cm precision.
// ---------------------------------------------------------------------------

const TMAP_MAGIC: &[u8; 4] = b"TMP1";

fn put_u16(o: &mut Vec<u8>, v: usize) {
    o.extend_from_slice(&(v.min(u16::MAX as usize) as u16).to_le_bytes());
}
fn put_u32(o: &mut Vec<u8>, v: usize) {
    o.extend_from_slice(&(v as u32).to_le_bytes());
}
fn put_str(o: &mut Vec<u8>, s: &str) {
    let mut end = s.len().min(200);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    o.push(end as u8);
    o.extend_from_slice(&s.as_bytes()[..end]);
}
fn put_ll(o: &mut Vec<u8>, p: V3) {
    let (lon, lat) = crate::camera::vec_to_lonlat(p);
    o.extend_from_slice(&((lon.to_degrees() * 1e7).round() as i32).to_le_bytes());
    o.extend_from_slice(&((lat.to_degrees() * 1e7).round() as i32).to_le_bytes());
}

pub fn encode(d: &SystemData) -> Vec<u8> {
    let mut o = Vec::with_capacity(d.point_count() * 8 + 4096);
    o.extend_from_slice(TMAP_MAGIC);
    put_u16(&mut o, d.lines.len());
    for l in &d.lines {
        o.extend_from_slice(&l.color);
        put_str(&mut o, &l.badge);
        put_str(&mut o, &l.name);
        put_u32(&mut o, l.paths.len());
        for p in &l.paths {
            put_u32(&mut o, p.len());
            for &v in p {
                put_ll(&mut o, v);
            }
        }
    }
    put_u32(&mut o, d.stations.len());
    for s in &d.stations {
        put_str(&mut o, &s.name);
        put_ll(&mut o, s.pos);
        o.push(s.lines.len().min(255) as u8);
        for &li in s.lines.iter().take(255) {
            put_u16(&mut o, li);
        }
    }
    o
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.b.len()).ok_or("file ends early")?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<usize, String> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]) as usize)
    }
    fn u32(&mut self) -> Result<usize, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    }
    fn i32(&mut self) -> Result<i32, String> {
        let b = self.take(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn string(&mut self) -> Result<String, String> {
        let n = self.u8()? as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
    fn ll(&mut self) -> Result<V3, String> {
        let lon = self.i32()? as f64 * 1e-7;
        let lat = self.i32()? as f64 * 1e-7;
        Ok(lonlat_to_vec(lon.to_radians(), lat.to_radians()))
    }
}

pub fn decode(bytes: &[u8]) -> Result<SystemData, String> {
    if bytes.len() < 4 || &bytes[..4] != TMAP_MAGIC {
        return Err("not a RustyMaps transit file".into());
    }
    let mut r = Reader { b: bytes, at: 4 };
    let mut d = SystemData::default();
    for _ in 0..r.u16()? {
        let c = r.take(3)?;
        let color = [c[0], c[1], c[2]];
        let badge = r.string()?;
        let name = r.string()?;
        let mut paths = Vec::new();
        for _ in 0..r.u32()? {
            let n = r.u32()?;
            if n > 5_000_000 {
                return Err("corrupt file".into());
            }
            let mut p = Vec::with_capacity(n);
            for _ in 0..n {
                p.push(r.ll()?);
            }
            paths.push(p);
        }
        d.lines.push(TLine { name, badge, color, paths });
    }
    for _ in 0..r.u32()? {
        let name = r.string()?;
        let pos = r.ll()?;
        let n = r.u8()? as usize;
        let mut lines = Vec::with_capacity(n);
        for _ in 0..n {
            lines.push(r.u16()?);
        }
        d.stations.push(Station { name, pos, lines });
    }
    d.finish();
    Ok(d)
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
            (!rf.is_empty() && rf == k.0)
                || (!k.1.is_empty() && (name_raw.to_lowercase().contains(&k.1.to_lowercase()) || name_any.to_lowercase().contains(&k.1.to_lowercase())))
        });
        let (key, color, badge) = match known {
            Some(k) => (
                format!("known:{}:{}", k.0, k.1),
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
    fn compact_format_round_trips() {
        let d = parse(&SYSTEMS[2], SAMPLE).unwrap();
        let bytes = encode(&d);
        let back = decode(&bytes).unwrap();
        assert_eq!(back.lines.len(), d.lines.len());
        assert_eq!(back.stations.len(), d.stations.len());
        assert_eq!(back.lines[0].badge, "G");
        assert_eq!(back.lines[0].color, [255, 149, 0]);
        assert_eq!(back.lines[0].paths[0].len(), 3);
        let (a, b) = (back.lines[0].paths[0][1], d.lines[0].paths[0][1]);
        let err = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt() * 6_371_000.0;
        assert!(err < 0.05, "{err} m");
        let names: Vec<&str> = back.stations.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Shibuya"));
        assert!(back.radius > 0.0);
        assert!(decode(&bytes[..bytes.len() - 3]).is_err());
        assert!(decode(b"nope").is_err());
    }

    #[test]
    fn known_colours_are_keyed_by_ref_so_ny_lines_stay_apart() {
        let ny = SYSTEMS.iter().find(|s| s.id == "newyork").unwrap();
        let text = r##"{"elements":[
          {"type":"relation","id":1,"tags":{"route":"subway","ref":"A","name":"A Train"},"members":[{"type":"way","ref":1,"role":"","geometry":[{"lat":40.7,"lon":-74.0},{"lat":40.8,"lon":-73.9}]}]},
          {"type":"relation","id":2,"tags":{"route":"subway","ref":"1","name":"1 Train"},"members":[{"type":"way","ref":2,"role":"","geometry":[{"lat":40.7,"lon":-74.0},{"lat":40.9,"lon":-73.9}]}]}
        ]}"##;
        let d = parse(ny, text).unwrap();
        assert_eq!(d.lines.len(), 2);
        assert_ne!(d.lines[0].color, d.lines[1].color);
    }

    #[test]
    fn every_system_is_well_formed() {
        let mut ids = std::collections::HashSet::new();
        for c in SYSTEMS {
            assert!(ids.insert(c.id), "duplicate id {}", c.id);
            let [s, w, n, e] = c.bbox;
            assert!(s < n && w < e && (-90.0..=90.0).contains(&s) && (-180.0..=180.0).contains(&w), "{}", c.id);
            assert!(c.home.0 >= s && c.home.0 <= n && c.home.1 >= w && c.home.1 <= e, "{} home outside its box", c.id);
            assert!(!c.filters.is_empty());
        }
        assert!(SYSTEMS.len() >= 25);
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
