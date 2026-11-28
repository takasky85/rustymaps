//! Turns one decoded vector tile into GPU-ready geometry plus label data.
//!
//! Runs on worker threads, never on the render thread.
//!
//! * Polygons are triangulated with `earcutr` and any triangle whose edges span
//!   more than ~3 degrees is split, so that on the globe flat triangles do not
//!   cut through the sphere at low zoom.
//! * Lines become instances (one per segment); the vertex shader expands each
//!   into a screen-space quad of the right pixel width.
//! * Points from the `place` / `water_name` layers become labels, drawn by egui.

use crate::mvt::{self, Feature, Layer};
use crate::style as st;
use crate::tile_id::TileId;

/// The most places of interest and house numbers kept per tile, so a dense city centre cannot make
/// one tile heavy.
const MAX_POI: usize = 500;
const MAX_HOUSES: usize = 1200;

/// Tile coordinate extent that all geometry is normalized to.
pub const EXTENT: f32 = 4096.0;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FillVertex {
    pub x: f32,
    pub y: f32,
    pub style: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct LineInst {
    pub ax: f32,
    pub ay: f32,
    pub bx: f32,
    pub by: f32,
    pub style: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Continent,
    Ocean,
    Sea,
    Country,
    State,
    City,
    Town,
    Village,
    Suburb,
    Island,
    /// A street name, running along its road.
    Street,
    /// A place of interest (cafe, museum, bus stop...), drawn as a dot with its name.
    Poi,
    HouseNumber,
    /// A mountain peak, with its height.
    Peak,
    Airport,
}

#[derive(Clone, Debug)]
pub struct Label {
    /// Position in normalized world Web Mercator (0..1, y down).
    pub mx: f64,
    pub my: f64,
    pub text: String,
    pub kind: Kind,
    pub rank: u32,
    /// Street names only: direction of the road (unit vector in mercator, y down) and the length
    /// of the road piece in the tile (mercator units). Zero for everything else.
    pub dir: [f32; 2],
    pub span: f32,
    /// Places of interest only: which group (see `detail::poi_group`).
    pub cat: u8,
}

#[derive(Default)]
pub struct TileMesh {
    pub fills: Vec<FillVertex>,
    pub lines: Vec<LineInst>,
    pub labels: Vec<Label>,
}

struct Builder {
    id: TileId,
    /// Longest allowed edge, in tile units (~3 degrees of arc).
    max_edge: f32,
    fills: [Vec<FillVertex>; st::COUNT],
    lines: [Vec<LineInst>; st::COUNT],
    labels: Vec<Label>,
}

pub fn build_mesh(id: TileId, data: &[u8]) -> Result<TileMesh, String> {
    let layers = mvt::decode_tile(data)?;
    let mut b = Builder {
        id,
        max_edge: EXTENT / 360.0 * 3.0 * (1u64 << id.z) as f32,
        fills: std::array::from_fn(|_| Vec::new()),
        lines: std::array::from_fn(|_| Vec::new()),
        labels: Vec::new(),
    };
    for layer in &layers {
        b.layer(layer);
    }
    let mut mesh = TileMesh::default();
    for v in b.fills {
        mesh.fills.extend(v);
    }
    for v in b.lines {
        mesh.lines.extend(v);
    }
    mesh.labels = b.labels;
    Ok(mesh)
}

fn fill_style(layer: &Layer, f: &Feature) -> Option<u32> {
    let class = layer.str_prop(f, "class").unwrap_or("");
    match layer.name.as_str() {
        "water" => Some(st::WATER),
        "park" => Some(st::PARK),
        "building" => Some(st::BUILDING),
        "landcover" => match class {
            "ice" | "glacier" => Some(st::COVER_ICE),
            "sand" => Some(st::COVER_SAND),
            "farmland" | "farm" => Some(st::COVER_FARM),
            "grass" => Some(st::COVER_GRASS),
            "wetland" => Some(st::COVER_WETLAND),
            "wood" | "forest" => Some(st::COVER_WOOD),
            _ => None,
        },
        "landuse" => match class {
            "" => None,
            "residential" | "suburb" | "neighbourhood" => Some(st::LANDUSE_RES),
            _ => Some(st::LANDUSE_OTHER),
        },
        _ => None,
    }
}

fn line_style(layer: &Layer, f: &Feature) -> Option<u32> {
    match layer.name.as_str() {
        "waterway" => Some(st::WATERWAY),
        "boundary" => {
            if layer.num_prop(f, "maritime") == Some(1.0) {
                return None;
            }
            match layer.num_prop(f, "admin_level") {
                Some(l) if l <= 2.0 => Some(st::BORDER_COUNTRY),
                Some(l) if l <= 4.0 => Some(st::BORDER_STATE),
                _ => None,
            }
        }
        "transportation" => match layer.str_prop(f, "class")? {
            "motorway" => Some(st::ROAD_MOTORWAY),
            "trunk" => Some(st::ROAD_TRUNK),
            "primary" => Some(st::ROAD_PRIMARY),
            "secondary" => Some(st::ROAD_SECONDARY),
            "tertiary" => Some(st::ROAD_TERTIARY),
            "minor" => Some(st::ROAD_MINOR),
            "service" | "busway" | "bus_guideway" => Some(st::ROAD_SERVICE),
            "track" | "path" => Some(st::PATH),
            "rail" | "transit" => Some(st::RAIL),
            "ferry" => Some(st::FERRY),
            _ => None,
        },
        _ => None,
    }
}

fn signed_area(ring: &[(i32, i32)]) -> f64 {
    let mut sum = 0i64;
    for i in 0..ring.len() {
        let (x0, y0) = ring[i];
        let (x1, y1) = ring[(i + 1) % ring.len()];
        sum += x0 as i64 * y1 as i64 - x1 as i64 * y0 as i64;
    }
    sum as f64 * 0.5
}

fn pick_name(layer: &Layer, f: &Feature) -> Option<String> {
    for key in ["name_en", "name:latin", "name"] {
        if let Some(s) = layer.str_prop(f, key) {
            let s = s.trim();
            // The bundled egui fonts only cover Latin scripts.
            if !s.is_empty() && s.chars().all(|c| (c as u32) < 0x250) {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn dist2(a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
    dx * dx + dy * dy
}

fn mid(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5]
}

impl Builder {
    fn layer(&mut self, layer: &Layer) {
        let scale = EXTENT / layer.extent.max(1) as f32;
        match layer.name.as_str() {
            "water" | "landcover" | "landuse" | "park" | "building" => {
                for f in &layer.features {
                    if f.geom_type != 3 {
                        continue;
                    }
                    if let Some(style) = fill_style(layer, f) {
                        self.polygon(f, scale, style);
                    }
                }
            }
            "waterway" | "boundary" | "transportation" => {
                for f in &layer.features {
                    if f.geom_type != 2 {
                        continue;
                    }
                    if let Some(style) = line_style(layer, f) {
                        self.line(f, scale, style);
                    }
                }
            }
            "aeroway" => {
                for f in &layer.features {
                    if !matches!(layer.str_prop(f, "class"), Some("runway") | Some("taxiway") | Some("apron") | Some("helipad")) {
                        continue;
                    }
                    match f.geom_type {
                        3 => self.polygon(f, scale, st::AERO_FILL),
                        2 => self.line(f, scale, st::AERO_LINE),
                        _ => {}
                    }
                }
            }
            "place" | "water_name" => {
                for f in &layer.features {
                    self.label(layer, f, scale);
                }
            }
            "poi" => self.poi_labels(layer, scale),
            "housenumber" => self.house_labels(layer, scale),
            "mountain_peak" => self.peak_labels(layer, scale),
            "aerodrome_label" => self.airport_labels(layer, scale),
            "transportation_name" => {
                // street names only matter (and only fit) when zoomed in close
                if self.id.z >= 13 {
                    for f in &layer.features {
                        self.street_label(layer, f, scale);
                    }
                }
            }
            _ => {}
        }
    }

    fn polygon(&mut self, f: &Feature, scale: f32, style: u32) {
        let paths = f.paths();
        // Group rings into polygons: the first ring's winding marks "exterior";
        // opposite-winding rings are holes of the most recent exterior.
        let mut polys: Vec<Vec<usize>> = Vec::new();
        let mut ext_sign = 0.0f64;
        for (i, ring) in paths.iter().enumerate() {
            let a = signed_area(ring);
            if ring.len() < 3 || a == 0.0 {
                continue;
            }
            if polys.is_empty() {
                ext_sign = a.signum();
                polys.push(vec![i]);
            } else if a.signum() == ext_sign {
                polys.push(vec![i]);
            } else if let Some(last) = polys.last_mut() {
                last.push(i);
            }
        }
        for poly in polys {
            let mut data: Vec<f64> = Vec::new();
            let mut holes: Vec<usize> = Vec::new();
            let mut valid = true;
            for (k, &ri) in poly.iter().enumerate() {
                let mut pts: &[(i32, i32)] = &paths[ri];
                if pts.len() > 1 && pts.first() == pts.last() {
                    pts = &pts[..pts.len() - 1];
                }
                if pts.len() < 3 {
                    if k == 0 {
                        valid = false;
                        break;
                    }
                    continue;
                }
                if k > 0 {
                    holes.push(data.len() / 2);
                }
                for &(x, y) in pts {
                    data.push(x as f64);
                    data.push(y as f64);
                }
            }
            if !valid || data.len() < 6 {
                continue;
            }
            let Ok(idx) = earcutr::earcut(&data[..], &holes[..], 2) else {
                continue;
            };
            let p = |i: usize| [data[2 * i] as f32 * scale, data[2 * i + 1] as f32 * scale];
            for tri in idx.chunks_exact(3) {
                self.tri(p(tri[0]), p(tri[1]), p(tri[2]), style);
            }
        }
    }

    /// Pushes a triangle, splitting its longest edge until every edge is short enough.
    fn tri(&mut self, a: [f32; 2], b: [f32; 2], c: [f32; 2], style: u32) {
        let limit = self.max_edge * self.max_edge;
        let out = &mut self.fills[style as usize];
        let mut stack = vec![(a, b, c)];
        while let Some((a, b, c)) = stack.pop() {
            let (dab, dbc, dca) = (dist2(a, b), dist2(b, c), dist2(c, a));
            if dab.max(dbc).max(dca) <= limit {
                for p in [a, b, c] {
                    out.push(FillVertex { x: p[0], y: p[1], style });
                }
            } else if dab >= dbc && dab >= dca {
                let m = mid(a, b);
                stack.push((a, m, c));
                stack.push((m, b, c));
            } else if dbc >= dca {
                let m = mid(b, c);
                stack.push((a, b, m));
                stack.push((a, m, c));
            } else {
                let m = mid(c, a);
                stack.push((a, b, m));
                stack.push((m, b, c));
            }
        }
    }

    fn line(&mut self, f: &Feature, scale: f32, style: u32) {
        for path in f.paths() {
            for w in path.windows(2) {
                let a = [w[0].0 as f32 * scale, w[0].1 as f32 * scale];
                let b = [w[1].0 as f32 * scale, w[1].1 as f32 * scale];
                self.segment(a, b, style);
            }
        }
    }

    fn segment(&mut self, a: [f32; 2], b: [f32; 2], style: u32) {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-3 {
            return;
        }
        let n = (len / self.max_edge).ceil().max(1.0) as usize;
        let out = &mut self.lines[style as usize];
        for i in 0..n {
            let t0 = i as f32 / n as f32;
            let t1 = (i + 1) as f32 / n as f32;
            out.push(LineInst {
                ax: a[0] + dx * t0,
                ay: a[1] + dy * t0,
                bx: a[0] + dx * t1,
                by: a[1] + dy * t1,
                style,
            });
        }
    }

    fn label(&mut self, layer: &Layer, f: &Feature, scale: f32) {
        if f.geom_type != 1 {
            return;
        }
        let class = layer.str_prop(f, "class");
        let kind = if layer.name == "place" {
            match class {
                Some("continent") => Kind::Continent,
                Some("country") => Kind::Country,
                Some("state") | Some("province") => Kind::State,
                Some("city") => Kind::City,
                Some("town") => Kind::Town,
                Some("village") | Some("hamlet") => Kind::Village,
                Some("suburb") | Some("quarter") | Some("neighbourhood") | Some("borough") => Kind::Suburb,
                Some("island") => Kind::Island,
                _ => return,
            }
        } else {
            match class {
                Some("ocean") => Kind::Ocean,
                Some("sea") => Kind::Sea,
                _ => return,
            }
        };
        let Some(text) = pick_name(layer, f) else {
            return;
        };
        let paths = f.paths();
        let Some(&(px, py)) = paths.first().and_then(|p| p.first()) else {
            return;
        };
        let (lx, ly) = (px as f32 * scale, py as f32 * scale);
        // Points in the tile's buffer zone belong to the neighbouring tile.
        if lx < 0.0 || ly < 0.0 || lx >= EXTENT || ly >= EXTENT {
            return;
        }
        let n = (1u64 << self.id.z) as f64;
        self.labels.push(Label {
            mx: (self.id.x as f64 + lx as f64 / EXTENT as f64) / n,
            my: (self.id.y as f64 + ly as f64 / EXTENT as f64) / n,
            text,
            kind,
            rank: layer.num_prop(f, "rank").map(|r| r as u32).unwrap_or(50),
            dir: [0.0, 0.0],
            span: 0.0,
            cat: 0,
        });
    }

    /// Where a point feature sits (normalized mercator), or `None` when it is in the tile's buffer
    /// zone (it belongs to the neighbouring tile).
    fn point_at(&self, f: &Feature, scale: f32) -> Option<(f64, f64)> {
        if f.geom_type != 1 {
            return None;
        }
        let paths = f.paths();
        let &(px, py) = paths.first().and_then(|p| p.first())?;
        let (lx, ly) = (px as f32 * scale, py as f32 * scale);
        if lx < 0.0 || ly < 0.0 || lx >= EXTENT || ly >= EXTENT {
            return None;
        }
        let n = (1u64 << self.id.z) as f64;
        Some(((self.id.x as f64 + lx as f64 / EXTENT as f64) / n, (self.id.y as f64 + ly as f64 / EXTENT as f64) / n))
    }

    fn push_point(&mut self, at: (f64, f64), text: String, kind: Kind, rank: u32, cat: u8) {
        self.labels.push(Label { mx: at.0, my: at.1, text, kind, rank, dir: [0.0, 0.0], span: 0.0, cat });
    }

    /// Places of interest, the most important MAX_POI per tile.
    fn poi_labels(&mut self, layer: &Layer, scale: f32) {
        // a tile this zoomed out has no room for them; they only show from about zoom 15
        if self.id.z < 12 {
            return;
        }
        let mut found: Vec<(u32, (f64, f64), String, u8)> = Vec::new();
        for f in &layer.features {
            let Some(text) = pick_name(layer, f) else { continue };
            let Some(at) = self.point_at(f, scale) else { continue };
            let class = layer.str_prop(f, "class").unwrap_or("");
            let rank = layer.num_prop(f, "rank").map(|r| r.max(1.0) as u32).unwrap_or(100);
            found.push((rank, at, text, crate::detail::poi_group(class)));
        }
        found.sort_by_key(|p| p.0);
        found.truncate(MAX_POI);
        for (rank, at, text, cat) in found {
            self.push_point(at, text, Kind::Poi, rank, cat);
        }
    }

    fn house_labels(&mut self, layer: &Layer, scale: f32) {
        if self.id.z < 14 {
            return;
        }
        let mut n = 0;
        for f in &layer.features {
            let Some(num) = layer.str_prop(f, "housenumber").map(str::trim).filter(|t| !t.is_empty() && t.len() <= 8) else { continue };
            let Some(at) = self.point_at(f, scale) else { continue };
            self.push_point(at, num.to_string(), Kind::HouseNumber, 100, 0);
            n += 1;
            if n >= MAX_HOUSES {
                break;
            }
        }
    }

    fn peak_labels(&mut self, layer: &Layer, scale: f32) {
        for f in &layer.features {
            let Some(name) = pick_name(layer, f) else { continue };
            let Some(at) = self.point_at(f, scale) else { continue };
            let text = match layer.num_prop(f, "ele") {
                Some(m) if m > 0.0 => format!("{name}  {} m", m.round() as i64),
                _ => name,
            };
            // higher peaks are more important: rank 0 at 4000 m and up
            let ele = layer.num_prop(f, "ele").unwrap_or(0.0);
            let rank = (40.0 - (ele / 100.0).min(40.0)).max(0.0) as u32;
            self.push_point(at, text, Kind::Peak, rank, 0);
        }
    }

    fn airport_labels(&mut self, layer: &Layer, scale: f32) {
        for f in &layer.features {
            let Some(name) = pick_name(layer, f) else { continue };
            let Some(at) = self.point_at(f, scale) else { continue };
            let text = match layer.str_prop(f, "iata").map(str::trim).filter(|c| !c.is_empty()) {
                Some(code) => format!("{name} ({code})"),
                None => name,
            };
            let rank = layer.num_prop(f, "rank").map(|r| r as u32).unwrap_or(50);
            self.push_point(at, text, Kind::Airport, rank, 0);
        }
    }

    /// A street name at the middle of a named road, turned along it.
    fn street_label(&mut self, layer: &Layer, f: &Feature, scale: f32) {
        if f.geom_type != 2 {
            return;
        }
        let rank = match layer.str_prop(f, "class") {
            Some("motorway") => 0,
            Some("trunk") => 1,
            Some("primary") => 2,
            Some("secondary") => 3,
            Some("tertiary") => 4,
            Some("minor") => 5,
            Some("service") | Some("pedestrian") | Some("path") | Some("track") => 6,
            _ => return,
        };
        let Some(text) = pick_name(layer, f) else { return };
        let paths = f.paths();
        // the longest piece of the road
        let Some(path) = paths.iter().max_by_key(|p| p.len()) else { return };
        if path.len() < 2 {
            return;
        }
        let pts: Vec<[f32; 2]> = path.iter().map(|&(x, y)| [x as f32 * scale, y as f32 * scale]).collect();
        let total: f32 = pts.windows(2).map(|w| dist2(w[0], w[1]).sqrt()).sum();
        if total <= 1.0 {
            return;
        }
        // walk to the middle
        let mut left = total * 0.5;
        let mut at = pts[0];
        let mut dir = [1.0f32, 0.0];
        for w in pts.windows(2) {
            let seg = dist2(w[0], w[1]).sqrt();
            if seg <= 1e-6 {
                continue;
            }
            dir = [(w[1][0] - w[0][0]) / seg, (w[1][1] - w[0][1]) / seg];
            if left <= seg {
                at = [w[0][0] + dir[0] * left, w[0][1] + dir[1] * left];
                break;
            }
            left -= seg;
            at = w[1];
        }
        // Middles in the tile's buffer zone belong to the neighbouring tile.
        if at[0] < 0.0 || at[1] < 0.0 || at[0] >= EXTENT || at[1] >= EXTENT {
            return;
        }
        let n = (1u64 << self.id.z) as f64;
        self.labels.push(Label {
            mx: (self.id.x as f64 + at[0] as f64 / EXTENT as f64) / n,
            my: (self.id.y as f64 + at[1] as f64 / EXTENT as f64) / n,
            text,
            kind: Kind::Street,
            rank,
            dir,
            span: (total as f64 / EXTENT as f64 / n) as f32,
            cat: 0,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mvt::test_enc::*;

    fn tri_area(v: &[FillVertex]) -> f64 {
        let mut total = 0.0;
        for t in v.chunks_exact(3) {
            let (a, b, c) = (t[0], t[1], t[2]);
            total += (((b.x - a.x) * (c.y - a.y) - (c.x - a.x) * (b.y - a.y)) as f64 * 0.5).abs();
        }
        total
    }

    #[test]
    fn big_polygon_is_split_on_low_zoom_tiles() {
        let sq = vec![(0, 0), (4096, 0), (4096, 4096), (0, 4096)];
        let data = tile(
            "water",
            vec![Feat { geom: Geom::Polygon(vec![sq]), props: vec![("class", Val::S("ocean"))] }],
        );
        let mesh = build_mesh(TileId { z: 0, x: 0, y: 0 }, &data).unwrap();
        assert!(mesh.fills.len() % 3 == 0);
        // Splitting must conserve area.
        let area = tri_area(&mesh.fills);
        assert!((area - 4096.0 * 4096.0).abs() < 1.0, "area {area}");
        // ...and no edge may exceed ~3 degrees (34.13 units at z0).
        let limit = EXTENT / 360.0 * 3.0 * 1.0001;
        for t in mesh.fills.chunks_exact(3) {
            for (p, q) in [(0, 1), (1, 2), (2, 0)] {
                let d = ((t[p].x - t[q].x).powi(2) + (t[p].y - t[q].y).powi(2)).sqrt();
                assert!(d <= limit, "edge {d} > {limit}");
            }
        }
        assert!(mesh.fills.iter().all(|v| v.style == st::WATER));
        assert!(mesh.fills.len() > 10_000, "expected many small triangles");
    }

    #[test]
    fn high_zoom_polygon_is_not_split() {
        let sq = vec![(100, 100), (3000, 100), (3000, 3000), (100, 3000)];
        let data = tile(
            "park",
            vec![Feat { geom: Geom::Polygon(vec![sq]), props: vec![] }],
        );
        let mesh = build_mesh(TileId { z: 12, x: 655, y: 1583 }, &data).unwrap();
        assert_eq!(mesh.fills.len(), 6, "a quad is two triangles");
        assert!(mesh.fills.iter().all(|v| v.style == st::PARK));
    }

    #[test]
    fn lines_get_styles_and_are_subdivided() {
        let data = tile(
            "transportation",
            vec![
                Feat { geom: Geom::Line(vec![(0, 0), (4096, 0)]), props: vec![("class", Val::S("motorway"))] },
                Feat { geom: Geom::Line(vec![(0, 10), (100, 10)]), props: vec![("class", Val::S("minor"))] },
                Feat { geom: Geom::Line(vec![(0, 20), (100, 20)]), props: vec![("class", Val::S("nonsense"))] },
            ],
        );
        let mesh = build_mesh(TileId { z: 0, x: 0, y: 0 }, &data).unwrap();
        let motorway = mesh.lines.iter().filter(|l| l.style == st::ROAD_MOTORWAY).count();
        let minor = mesh.lines.iter().filter(|l| l.style == st::ROAD_MINOR).count();
        assert_eq!(motorway, 120, "4096 units / 3 degrees-worth (34.13) = exactly 120");
        assert_eq!(minor, 3, "100 / 34.13 rounded up");
        assert_eq!(mesh.lines.len(), motorway + minor, "unknown class is dropped");
        // Lines are sorted by style id: minor (17) before motorway (22).
        assert!(mesh.lines.first().unwrap().style < mesh.lines.last().unwrap().style);
    }

    #[test]
    fn places_of_interest_house_numbers_peaks_and_airports_become_labels() {
        use crate::mvt::test_enc::*;
        let pt = |x, y| Geom::Point(x, y);
        let poi = tile(
            "poi",
            vec![
                Feat { geom: pt(100, 100), props: vec![("name", Val::S("Blue Cafe")), ("class", Val::S("cafe")), ("rank", Val::N(7.0))] },
                Feat { geom: pt(200, 200), props: vec![("name", Val::S("General Hospital")), ("class", Val::S("hospital")), ("rank", Val::N(2.0))] },
                Feat { geom: pt(300, 300), props: vec![("class", Val::S("cafe"))] },
            ],
        );
        let m = build_mesh(TileId { z: 14, x: 2, y: 5 }, &poi).unwrap();
        assert_eq!(m.labels.len(), 2, "the one without a name is dropped");
        assert_eq!(m.labels[0].text, "General Hospital", "most important first");
        assert!(m.labels.iter().all(|l| l.kind == Kind::Poi));
        assert_eq!((m.labels[0].cat, m.labels[1].cat), (3, 0));
        // not kept on a zoomed-out tile
        assert!(build_mesh(TileId { z: 8, x: 2, y: 5 }, &poi).unwrap().labels.is_empty());

        let houses = tile("housenumber", vec![Feat { geom: pt(50, 50), props: vec![("housenumber", Val::S("221B"))] }, Feat { geom: pt(60, 60), props: vec![] }]);
        let m = build_mesh(TileId { z: 14, x: 2, y: 5 }, &houses).unwrap();
        assert_eq!(m.labels.len(), 1);
        assert_eq!((m.labels[0].kind, m.labels[0].text.as_str()), (Kind::HouseNumber, "221B"));

        let peaks = tile("mountain_peak", vec![Feat { geom: pt(10, 10), props: vec![("name", Val::S("Mt Tam")), ("ele", Val::N(784.0))] }]);
        let m = build_mesh(TileId { z: 10, x: 1, y: 1 }, &peaks).unwrap();
        assert_eq!((m.labels[0].kind, m.labels[0].text.as_str()), (Kind::Peak, "Mt Tam  784 m"));

        let air = tile("aerodrome_label", vec![Feat { geom: pt(10, 10), props: vec![("name", Val::S("San Francisco Intl")), ("iata", Val::S("SFO"))] }]);
        let m = build_mesh(TileId { z: 10, x: 1, y: 1 }, &air).unwrap();
        assert_eq!((m.labels[0].kind, m.labels[0].text.as_str()), (Kind::Airport, "San Francisco Intl (SFO)"));
    }

    #[test]
    fn runways_and_aprons_are_drawn() {
        use crate::mvt::test_enc::*;
        let data = tile(
            "aeroway",
            vec![
                Feat { geom: Geom::Line(vec![(0, 0), (1000, 0)]), props: vec![("class", Val::S("runway"))] },
                Feat { geom: Geom::Polygon(vec![vec![(0, 100), (500, 100), (500, 600), (0, 600)]]), props: vec![("class", Val::S("apron"))] },
                Feat { geom: Geom::Line(vec![(0, 5), (1000, 5)]), props: vec![("class", Val::S("gate"))] },
            ],
        );
        let m = build_mesh(TileId { z: 12, x: 655, y: 1583 }, &data).unwrap();
        assert!(m.lines.iter().all(|l| l.style == st::AERO_LINE) && !m.lines.is_empty());
        assert_eq!(m.fills.len(), 6);
        assert!(m.fills.iter().all(|v| v.style == st::AERO_FILL));
    }

    #[test]
    fn borders_filter_by_admin_level_and_maritime() {
        let line = |level: f64, maritime: f64| Feat {
            geom: Geom::Line(vec![(0, 0), (10, 0)]),
            props: vec![("admin_level", Val::N(level)), ("maritime", Val::N(maritime))],
        };
        let data = tile("boundary", vec![line(2.0, 0.0), line(4.0, 0.0), line(6.0, 0.0), line(2.0, 1.0)]);
        let mesh = build_mesh(TileId { z: 5, x: 0, y: 0 }, &data).unwrap();
        assert_eq!(mesh.lines.iter().filter(|l| l.style == st::BORDER_COUNTRY).count(), 1);
        assert_eq!(mesh.lines.iter().filter(|l| l.style == st::BORDER_STATE).count(), 1);
        assert_eq!(mesh.lines.len(), 2);
    }

    #[test]
    fn street_names_sit_mid_road_and_follow_its_direction() {
        let data = tile(
            "transportation_name",
            vec![
                Feat {
                    geom: Geom::Line(vec![(1000, 2000), (3000, 2000)]),
                    props: vec![("class", Val::S("minor")), ("name", Val::S("Main Street"))],
                },
                Feat {
                    geom: Geom::Line(vec![(1000, 1000), (3000, 1000)]),
                    props: vec![("class", Val::S("minor"))],
                },
            ],
        );
        let mesh = build_mesh(TileId { z: 14, x: 3, y: 5 }, &data).unwrap();
        assert_eq!(mesh.labels.len(), 1, "unnamed roads get no label");
        let l = &mesh.labels[0];
        assert_eq!(l.kind, Kind::Street);
        assert!(l.dir[0].abs() > 0.99 && l.dir[1].abs() < 0.01);
        let n = 16384.0;
        assert!((l.mx - (3.0 + 2000.0 / 4096.0) / n).abs() < 1e-9, "{}", l.mx);
        assert!(l.span > 0.0);
        // not built for low zoom tiles
        let low = build_mesh(TileId { z: 8, x: 3, y: 5 }, &data).unwrap();
        assert!(low.labels.is_empty());
    }

    #[test]
    fn labels_are_positioned_in_world_mercator() {
        let data = tile(
            "place",
            vec![
                Feat {
                    geom: Geom::Point(2048, 2048),
                    props: vec![("class", Val::S("city")), ("name_en", Val::S("Midtown")), ("rank", Val::N(2.0))],
                },
                Feat {
                    geom: Geom::Point(10, 10),
                    props: vec![("class", Val::S("city")), ("name", Val::S("\u{6771}\u{4eac}"))],
                },
                Feat { geom: Geom::Point(10, 10), props: vec![("class", Val::S("highway")), ("name", Val::S("Nope"))] },
            ],
        );
        let mesh = build_mesh(TileId { z: 1, x: 1, y: 0 }, &data).unwrap();
        assert_eq!(mesh.labels.len(), 1, "non-latin and unknown classes are skipped");
        let l = &mesh.labels[0];
        assert_eq!(l.text, "Midtown");
        assert_eq!(l.kind, Kind::City);
        assert_eq!(l.rank, 2);
        assert!((l.mx - 0.75).abs() < 1e-9 && (l.my - 0.25).abs() < 1e-9, "{} {}", l.mx, l.my);
    }

    #[test]
    fn polygon_with_hole_keeps_hole_empty() {
        let outer = vec![(0, 0), (1000, 0), (1000, 1000), (0, 1000)];
        let hole = vec![(400, 400), (400, 600), (600, 600), (600, 400)];
        let data = tile(
            "water",
            vec![Feat { geom: Geom::Polygon(vec![outer, hole]), props: vec![] }],
        );
        let mesh = build_mesh(TileId { z: 14, x: 0, y: 0 }, &data).unwrap();
        let area = tri_area(&mesh.fills);
        assert!((area - (1_000_000.0 - 40_000.0)).abs() < 1.0, "area {area}");
    }

    #[test]
    fn empty_and_unknown_layers_are_fine() {
        let mesh = build_mesh(TileId { z: 3, x: 1, y: 1 }, &[]).unwrap();
        assert!(mesh.fills.is_empty() && mesh.lines.is_empty() && mesh.labels.is_empty());
        let data = tile("housenumber", vec![Feat { geom: Geom::Point(1, 1), props: vec![] }]);
        assert!(build_mesh(TileId { z: 14, x: 1, y: 1 }, &data).unwrap().labels.is_empty());
    }
}
