//! Transit overlays: route geometry from OpenStreetMap (Overpass), kept as small offline files in the
//! data folder (`data/transit/<id>.tmap`), and drawn in a subway-map style: thick colour-coded lines
//! with white casings, lettered roundels at line ends, and station circles and names that only
//! appear once they are spaced out enough to read. The lines follow the real tracks; only the styling
//! is "subway map".
//!
//! Files are used as they are (no expiry). A system whose file is missing is downloaded once, patiently,
//! from public Overpass servers and saved; `rustymaps --fetch-transit <dir>` does the same for all
//! systems in one go (that is what the "Data" GitHub workflow runs).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Stroke};

use crate::camera::{add, cross, dot, lonlat_to_vec, normalize, scale, Camera, Projector, FOV_Y, V3};
use crate::tiles::USER_AGENT;
use crate::transit_data::{self, SystemData, SYSTEMS};

#[derive(Clone, PartialEq)]
pub enum Status {
    Off,
    Loading,
    Ready,
    Failed(String),
}

type Pending = Arc<Mutex<Option<Result<SystemData, String>>>>;

struct Slot {
    enabled: bool,
    status: Status,
    data: Option<Arc<SystemData>>,
    pending: Pending,
    /// A saved `.tmap` exists in the data folder.
    offline: bool,
}

pub struct TransitManager {
    ctx: egui::Context,
    dir: PathBuf,
    slots: Vec<Slot>,
}

fn tmap_path(dir: &Path, id: &str) -> PathBuf {
    dir.join("transit").join(format!("{id}.tmap"))
}

impl TransitManager {
    pub fn new(ctx: &egui::Context, data_dir: PathBuf) -> Self {
        let slots = SYSTEMS
            .iter()
            .map(|c| Slot {
                enabled: false,
                status: Status::Off,
                data: None,
                pending: Arc::new(Mutex::new(None)),
                offline: tmap_path(&data_dir, c.id).is_file(),
            })
            .collect();
        TransitManager { ctx: ctx.clone(), dir: data_dir, slots }
    }

    pub fn enabled(&self, i: usize) -> bool {
        self.slots[i].enabled
    }

    pub fn status(&self, i: usize) -> Status {
        self.slots[i].status.clone()
    }

    /// True when the system is saved in the data folder (no download needed).
    pub fn offline(&self, i: usize) -> bool {
        self.slots[i].offline
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn set_enabled(&mut self, i: usize, on: bool) {
        let slot = &mut self.slots[i];
        slot.enabled = on;
        if on && slot.data.is_none() && slot.status != Status::Loading {
            slot.status = Status::Loading;
            let pending = slot.pending.clone();
            let ctx = self.ctx.clone();
            let dir = self.dir.clone();
            let _ = std::thread::Builder::new().name(format!("transit-{}", SYSTEMS[i].id)).spawn(move || {
                let res = load(i, &dir);
                *pending.lock().unwrap() = Some(res);
                ctx.request_repaint();
            });
        }
    }

    /// Picks up finished loads; call once per frame.
    pub fn poll(&mut self) {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            let done = slot.pending.lock().unwrap().take();
            match done {
                Some(Ok(d)) => {
                    slot.data = Some(Arc::new(d));
                    slot.status = Status::Ready;
                    slot.offline = tmap_path(&self.dir, SYSTEMS[i].id).is_file();
                }
                Some(Err(e)) => {
                    eprintln!("transit: {e}");
                    slot.status = Status::Failed(e);
                }
                None => {}
            }
        }
    }

    /// A marker for every enabled system whose map is saved in the data folder.
    pub fn markers(&self) -> Vec<Marker> {
        self.slots
            .iter()
            .zip(SYSTEMS)
            .filter(|(s, _)| s.enabled && s.offline)
            .map(|(_, c)| Marker { pos: lonlat_to_vec(c.home.1.to_radians(), c.home.0.to_radians()), title: c.title, home: c.home })
            .collect()
    }

    /// Stations of the switched-on systems whose name contains `query` (case-insensitive):
    /// (station name, system title, position). One entry per name and system.
    pub fn find_stations(&self, query: &str, limit: usize) -> Vec<(String, &'static str, V3)> {
        let q = query.trim().to_lowercase();
        let mut out: Vec<(String, &'static str, V3)> = Vec::new();
        if q.len() < 2 {
            return out;
        }
        for (slot, cfg) in self.slots.iter().zip(SYSTEMS) {
            let (true, Some(d)) = (slot.enabled, slot.data.as_ref()) else { continue };
            for st in &d.stations {
                if out.len() >= limit {
                    return out;
                }
                if !st.name.is_empty()
                    && st.name.to_lowercase().contains(&q)
                    && !out.iter().any(|o| o.0 == st.name && o.1 == cfg.title)
                {
                    out.push((st.name.clone(), cfg.title, st.pos));
                }
            }
        }
        out
    }

    /// Data of all enabled, loaded systems.
    pub fn visible(&self) -> Vec<Arc<SystemData>> {
        self.slots.iter().filter(|s| s.enabled).filter_map(|s| s.data.clone()).collect()
    }
}

/// Reads the saved file, or downloads (once) and saves it.
fn load(i: usize, dir: &Path) -> Result<SystemData, String> {
    let cfg = &SYSTEMS[i];
    let path = tmap_path(dir, cfg.id);
    if let Ok(bytes) = std::fs::read(&path) {
        return transit_data::decode(&bytes).map_err(|e| format!("{}: {e}", path.display()));
    }
    let text = fetch(&transit_data::overpass_query(cfg))?;
    let data = transit_data::parse(cfg, &text).map_err(|e| format!("could not read the {} data: {e}", cfg.title))?;
    drop(text);
    if data.lines.is_empty() {
        return Err(format!("OpenStreetMap returned no {} lines (tagging may differ from what the query expects)", cfg.title));
    }
    if std::fs::create_dir_all(path.parent().unwrap_or(dir)).is_ok() {
        let _ = std::fs::write(&path, transit_data::encode(&data));
    }
    // Use what was just saved, so what you see is exactly what an offline start would show.
    Ok(data)
}

/// Public Overpass servers, tried in order. Any of them failing just moves on to the next.
const ENDPOINTS: [&str; 3] = [
    "https://overpass-api.de/api/interpreter",
    "https://overpass.private.coffee/api/interpreter",
    "https://overpass.kumi.systems/api/interpreter",
];

/// Only one Overpass request at a time, so switching on several systems is polite to the servers.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn snippet(text: &str) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.chars().take(160).collect()
}

fn post(agent: &ureq::Agent, url: &str, body: &str) -> Result<String, String> {
    let mut resp = agent
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .send(body)
        .map_err(|e| format!("network error: {e}"))?;
    let status = resp.status().as_u16();
    let text = resp
        .body_mut()
        .with_config()
        .limit(300 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| format!("could not read the answer: {e}"))?;
    if status != 200 {
        return Err(format!("HTTP {status} {}", snippet(&text)));
    }
    if !text.contains("\"elements\"") {
        return Err(format!("unexpected answer: {}", snippet(&text)));
    }
    Ok(text)
}

fn fetch(query: &str) -> Result<String, String> {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(200)))
        .user_agent(USER_AGENT)
        .http_status_as_error(false)
        .build()
        .into();
    let body = format!("data={}", transit_data::url_encode(query));
    let mut errors = Vec::new();
    for url in ENDPOINTS {
        match post(&agent, url, &body) {
            Ok(t) => return Ok(t),
            Err(e) => {
                let host = url.trim_start_matches("https://").split('/').next().unwrap_or(url);
                errors.push(format!("{host}: {e}"));
            }
        }
    }
    Err(errors.join(" | "))
}

/// Headless mode (`rustymaps --fetch-transit <dir> [id ...]`): downloads systems with patient retries and
/// writes `<dir>/<id>.tmap` plus `<dir>/fetch.log`. Systems that already have a file are skipped, so a
/// second run only fills the gaps. Used by the Data workflow; handy by hand when the servers are calm.
pub fn fetch_to_dir(dir: &Path, only: &[String]) {
    use std::io::Write;
    let _ = std::fs::create_dir_all(dir);
    let mut log = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("fetch.log")).ok();
    let mut say = |msg: String| {
        println!("{msg}");
        if let Some(f) = log.as_mut() {
            let _ = writeln!(f, "{msg}");
            let _ = f.flush();
        }
    };
    let mut failed = 0;
    for cfg in SYSTEMS.iter().filter(|c| only.is_empty() || only.iter().any(|o| o == c.id)) {
        let out = dir.join(format!("{}.tmap", cfg.id));
        if out.is_file() {
            say(format!("{}: already saved, skipping", cfg.id));
            continue;
        }
        let mut done = false;
        for attempt in 1..=5u64 {
            say(format!("{}: attempt {attempt}", cfg.id));
            match fetch(&transit_data::overpass_query(cfg)) {
                Ok(text) => {
                    let bytes = text.len();
                    match transit_data::parse(cfg, &text) {
                        Ok(d) if !d.lines.is_empty() => {
                            say(format!(
                                "{}: ok, {} lines, {} stations, {} points, {} MB downloaded",
                                cfg.id,
                                d.lines.len(),
                                d.stations.len(),
                                d.point_count(),
                                bytes / 1_000_000
                            ));
                            for l in &d.lines {
                                say(format!(
                                    "    {:<28} badge {:<3} #{:02x}{:02x}{:02x}  {} paths",
                                    l.name,
                                    l.badge,
                                    l.color[0],
                                    l.color[1],
                                    l.color[2],
                                    l.paths.len()
                                ));
                            }
                            let packed = transit_data::encode(&d);
                            say(format!("{}: saved {} KB", cfg.id, packed.len() / 1000));
                            let _ = std::fs::write(&out, packed);
                            done = true;
                        }
                        Ok(_) => say(format!("{}: answer contained no lines", cfg.id)),
                        Err(e) => say(format!("{}: could not parse: {e}", cfg.id)),
                    }
                }
                Err(e) => say(format!("{}: {e}", cfg.id)),
            }
            if done {
                break;
            }
            std::thread::sleep(Duration::from_secs(30 * attempt));
        }
        if !done {
            failed += 1;
            say(format!("{}: gave up", cfg.id));
        }
        std::thread::sleep(Duration::from_secs(8)); // be gentle between systems
    }
    say(format!("finished, {failed} system(s) missing"));
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

/// A metro symbol shown on the map where a city's transit map is available.
pub struct Marker {
    pub pos: V3,
    pub title: &'static str,
    /// (lat, lon) in degrees.
    pub home: (f64, f64),
}

/// A station name that main's label placer should position and collision-check.
pub struct StationLabel {
    pub at: Pos2,
    pub text: String,
    pub interchange: bool,
}

fn lerp(zf: f64, pts: &[(f64, f64)]) -> f32 {
    if zf <= pts[0].0 {
        return pts[0].1 as f32;
    }
    for w in pts.windows(2) {
        if zf <= w[1].0 {
            let t = (zf - w[0].0) / (w[1].0 - w[0].0);
            return (w[0].1 + (w[1].1 - w[0].1) * t) as f32;
        }
    }
    pts[pts.len() - 1].1 as f32
}

/// Zoom levels: the lines fade in between `MAP_FROM` and `MAP_FULL` while the city's metro symbol
/// fades out; roundels and stations come in once the map is solid.
pub const MAP_FROM: f64 = 9.0;
pub const MAP_FULL: f64 = 10.0;
pub const STATIONS_FROM: f64 = 10.0;
pub const ROUNDELS_FROM: f64 = 10.5;

/// 0 = metro symbol only, 1 = full map (smooth cross-fade).
pub fn map_fade(zf: f64) -> f32 {
    let t = ((zf - MAP_FROM) / (MAP_FULL - MAP_FROM)).clamp(0.0, 1.0);
    (t * t * (3.0 - 2.0 * t)) as f32
}

/// Line width in logical pixels at a zoom level (the white casing adds to it). Slim when the
/// whole city is in view, fatter as you zoom in.
pub fn line_width(zf: f64) -> f32 {
    lerp(zf, &[(9.0, 2.0), (11.0, 3.6), (14.0, 7.0), (17.0, 12.0), (20.0, 16.0)])
}

fn fade(c: Color32, a: f32) -> Color32 {
    let [r, g, b, al] = c.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, (al as f32 * a).round().clamp(0.0, 255.0) as u8)
}

fn marker_radius(zf: f64) -> f32 {
    lerp(zf, &[(2.0, 6.0), (5.0, 9.0), (9.0, 12.0)])
}

/// Metro symbols (a blue disc with an "M") for cities whose map is available. They fade out as the
/// map itself fades in, and in as you leave the close-up globe.
pub fn draw_markers(painter: &egui::Painter, rect: egui::Rect, cam: &Camera, zf: f64, markers: &[Marker]) {
    let a = (1.0 - map_fade(zf)) * (((zf - 2.0) / 1.0).clamp(0.0, 1.0) as f32);
    if a <= 0.01 || markers.is_empty() {
        return;
    }
    let pr = cam.projector(rect.width() as f64, rect.height() as f64);
    let r = marker_radius(zf);
    let blue = fade(Color32::from_rgb(20, 80, 190), a);
    let white = fade(Color32::WHITE, a);
    let shadow = fade(Color32::from_black_alpha(90), a);
    for m in markers {
        if !pr.visible(m.pos) {
            continue;
        }
        let Some(s) = pr.project(m.pos) else { continue };
        let c = rect.min + egui::vec2(s[0] as f32, s[1] as f32);
        if !rect.expand(30.0).contains(c) {
            continue;
        }
        painter.circle_filled(c + egui::vec2(0.0, 1.5), r + 1.5, shadow);
        painter.circle(c, r, blue, Stroke::new(2.0_f32, white));
        painter.text(c, Align2::CENTER_CENTER, "M", FontId::proportional(r * 1.25), white);
        if zf >= 5.0 {
            let font = FontId::proportional(12.0);
            let at = c + egui::vec2(0.0, r + 9.0);
            let halo = fade(Color32::from_black_alpha(200), a);
            for (dx, dy) in [(-1.0_f32, 0.0_f32), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
                painter.text(at + egui::vec2(dx, dy), Align2::CENTER_CENTER, m.title, font.clone(), halo);
            }
            painter.text(at, Align2::CENTER_CENTER, m.title, font, white);
        }
    }
}

/// A station circle needs this much room (px to the nearest other station) to be drawn at all.
pub const MIN_STATION_GAP: f32 = 18.0;
/// ...and this much before its name is shown.
pub const MIN_NAME_GAP: f32 = 56.0;

/// Could any of this system be on screen? Cheap rejection before any per-point work.
fn may_be_visible(cam: &Camera, sys: &SystemData, aspect: f64) -> bool {
    if sys.radius <= 0.0 {
        return false;
    }
    let cp = cam.pos();
    let d = dot(cp, cp).sqrt();
    let horizon = (1.0 / d).clamp(-1.0, 1.0).acos();
    let nadir = normalize(cp);
    if dot(sys.center, nadir).clamp(-1.0, 1.0).acos() - sys.radius > horizon {
        return false;
    }
    if cam.alt < 0.2 {
        let half_diag = ((FOV_Y * 0.5).tan() * (1.0 + aspect * aspect).sqrt()).atan();
        let a = cam.pitch + half_diag;
        if a < 1.2 {
            let reach = cam.alt * (cam.pitch.cos() * a.tan() + cam.pitch.sin()) * 1.3 + 1e-4;
            if dot(sys.center, cam.dir()).clamp(-1.0, 1.0).acos() - sys.radius > reach {
                return false;
            }
        }
    }
    true
}

/// Nearest-neighbour distances (px) for a set of screen points, via a grid; capped at `cap`.
fn nearest_gaps(pts: &[Pos2], cap: f32) -> Vec<f32> {
    use std::collections::HashMap;
    let cell = cap.max(1.0);
    let key = |p: Pos2| ((p.x / cell).floor() as i32, (p.y / cell).floor() as i32);
    let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, p) in pts.iter().enumerate() {
        grid.entry(key(*p)).or_default().push(i);
    }
    pts.iter()
        .enumerate()
        .map(|(i, p)| {
            let (cx, cy) = key(*p);
            let mut best = cap;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if let Some(v) = grid.get(&(cx + dx, cy + dy)) {
                        for &j in v {
                            if j != i {
                                best = best.min(p.distance(pts[j]));
                            }
                        }
                    }
                }
            }
            best
        })
        .collect()
}

/// 0..1: how strongly a transit map is the subject of the view (a map is switched on, on screen and
/// faded in). The caller uses it to dim the base map and hide its labels.
pub fn focus(cam: &Camera, zf: f64, aspect: f64, systems: &[Arc<SystemData>]) -> f32 {
    let a = map_fade(zf);
    if a <= 0.0 || !systems.iter().any(|s| may_be_visible(cam, s, aspect)) {
        return 0.0;
    }
    a
}

/// A circle of `r_px` pixels lying flat on the ground around `p`: its outline projected through the
/// camera, so it squashes into an ellipse when the view is tilted. `units_per_px` is the ground size
/// of a pixel at the view centre (used only to pick a sensible probe distance).
fn flat_circle(pr: &Projector, rect: egui::Rect, p: V3, r_px: f32, units_per_px: f64) -> Option<Vec<Pos2>> {
    let east = normalize(cross([0.0, 1.0, 0.0], p));
    let north = cross(p, east);
    let eps = (units_per_px * 6.0).max(1e-12);
    let at = |q: V3| pr.project(q).map(|s| Pos2::new(s[0] as f32, s[1] as f32));
    let s0 = at(p)?;
    let se = at(add(p, scale(east, eps)))?;
    let sn = at(add(p, scale(north, eps)))?;
    let px_per_unit = ((s0.distance(se) + s0.distance(sn)) * 0.5) as f64 / eps;
    if px_per_unit <= 1e-9 {
        return None;
    }
    let g = r_px as f64 / px_per_unit;
    let mut pts = Vec::with_capacity(20);
    for k in 0..20 {
        let a = std::f64::consts::TAU * k as f64 / 20.0;
        let q = normalize(add(p, scale(add(scale(east, a.cos()), scale(north, a.sin())), g)));
        pts.push(at(q)? + rect.min.to_vec2());
    }
    Some(pts)
}

/// Everything `draw` needs besides the camera.
pub struct Opts<'a> {
    pub flat_stations: bool,
    pub trains: bool,
    /// Seconds, for moving the trains.
    pub now: f64,
    /// Where the user just clicked (screen position), to find which line was hit.
    pub pick: Option<Pos2>,
    /// The highlighted line: its system and its index there.
    pub selected: Option<(&'a Arc<SystemData>, usize)>,
}

pub struct Out {
    pub labels: Vec<StationLabel>,
    /// The line under `pick`, if any.
    pub hit: Option<(Arc<SystemData>, usize)>,
    /// Trains are moving, so the caller should keep repainting.
    pub animating: bool,
}

/// Simulated trains: not a timetable, just evenly spread trains shuttling along each line.
const TRAIN_SPEED_MS: f64 = 200.0;
const TRAIN_SPACING_KM: f64 = 2.5;
const TRAINS_FROM: f64 = 10.0;
const EARTH_RADIUS_M: f64 = 6_371_000.0;

fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    let t = if len2 <= 1e-9 { 0.0 } else { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) };
    p.distance(a + ab * t)
}

fn dist_to_polyline(p: Pos2, pts: &[Pos2]) -> f32 {
    pts.windows(2).map(|w| dist_to_segment(p, w[0], w[1])).fold(f32::INFINITY, f32::min)
}

/// Number of trains on a line of `len_rad` radians, and where train `k` of them is along a
/// there-and-back trip at time `now` (arc length in radians, 0..=len).
fn train_s(len_rad: f64, n: usize, k: usize, now: f64) -> f64 {
    let speed = TRAIN_SPEED_MS / EARTH_RADIUS_M;
    let u = ((k as f64 + 0.37) / n as f64 * 2.0 * len_rad + now * speed).rem_euclid(2.0 * len_rad);
    if u < len_rad {
        u
    } else {
        2.0 * len_rad - u
    }
}

fn train_count(len_rad: f64) -> usize {
    ((len_rad * EARTH_RADIUS_M / 1000.0 / TRAIN_SPACING_KM).round() as usize).clamp(2, 14)
}

pub fn draw(
    painter: &egui::Painter,
    rect: egui::Rect,
    cam: &Camera,
    zf: f64,
    systems: &[Arc<SystemData>],
    o: &Opts,
) -> Out {
    let mut out = Out { labels: Vec::new(), hit: None, animating: false };
    let fade_a = map_fade(zf);
    if fade_a <= 0.01 || systems.is_empty() {
        return out;
    }
    let (w, h) = (rect.width() as f64, rect.height() as f64);
    let pr = cam.projector(w, h);
    let to_screen = |p: V3| -> Option<Pos2> {
        if !pr.visible(p) {
            return None;
        }
        pr.project(p).map(|s| rect.min + egui::vec2(s[0] as f32, s[1] as f32))
    };
    let on_screen = rect.expand(80.0);

    let line_w = line_width(zf);
    let casing_extra = (line_w * 0.28).clamp(1.5, 4.0);
    let ink = Color32::from_rgb(35, 35, 45);
    let flat = o.flat_stations && cam.pitch > 0.12;
    let units_per_px = 2.0 * cam.alt * (FOV_Y * 0.5).tan() / h;
    let mut stations: Vec<(Pos2, &transit_data::Station)> = Vec::new();
    let mut trains: Vec<(Pos2, Color32)> = Vec::new();
    let mut best_hit: Option<(f32, Arc<SystemData>, usize)> = None;
    let train_a = (((zf - TRAINS_FROM) / 1.0).clamp(0.0, 1.0) as f32) * fade_a;

    for sys in systems {
        if !may_be_visible(cam, sys, w / h) {
            continue;
        }
        let sel = o.selected.and_then(|(a, i)| Arc::ptr_eq(a, sys).then_some(i));
        // 1. Project every path into on-screen runs.
        let mut runs: Vec<(usize, Vec<Pos2>)> = Vec::new();
        for (li, line) in sys.lines.iter().enumerate() {
            for path in &line.paths {
                let mut run: Vec<Pos2> = Vec::new();
                for &p in path {
                    match to_screen(p) {
                        Some(s) => {
                            if run.last().map_or(true, |l| l.distance(s) >= 0.8) {
                                run.push(s);
                            }
                        }
                        None => {
                            if run.len() >= 2 {
                                runs.push((li, std::mem::take(&mut run)));
                            }
                            run.clear();
                        }
                    }
                }
                if run.len() >= 2 {
                    runs.push((li, run));
                }
            }
        }
        // Keep runs whose bounding box touches the screen (a long straight segment can cross the
        // view with both of its vertices far outside it).
        runs.retain(|(_, r)| {
            let (mut lo, mut hi) = (r[0], r[0]);
            for p in r {
                lo = lo.min(*p);
                hi = hi.max(*p);
            }
            egui::Rect::from_min_max(lo, hi).intersects(on_screen)
        });

        // Which line was clicked?
        if let Some(pick) = o.pick {
            for (li, r) in &runs {
                let d = dist_to_polyline(pick, r);
                if d <= (line_w * 0.5 + 6.0).max(8.0) && best_hit.as_ref().map_or(true, |b| d < b.0) {
                    best_hit = Some((d, sys.clone(), *li));
                }
            }
        }

        // 2. All white casings first, then all colours: crossings look like a drawn map. With a line
        //    highlighted the others step back and the chosen one is drawn fatter, on top.
        let alpha_of = |li: usize| if sel.map_or(false, |s| s != li) { fade_a * 0.22 } else { fade_a };
        let width_of = |li: usize| if sel == Some(li) { line_w * 1.5 } else { line_w };
        for (li, r) in &runs {
            let casing = fade(Color32::from_rgba_unmultiplied(255, 255, 255, 240), alpha_of(*li));
            painter.add(egui::Shape::line(r.clone(), Stroke::new(width_of(*li) + casing_extra, casing)));
        }
        for pass in 0..2 {
            for (li, r) in &runs {
                if (sel == Some(*li)) != (pass == 1) {
                    continue;
                }
                let c = sys.lines[*li].color;
                painter.add(egui::Shape::line(r.clone(), Stroke::new(width_of(*li), fade(Color32::from_rgb(c[0], c[1], c[2]), alpha_of(*li)))));
            }
        }

        // 3. Roundels at the ends of each line's longest stretch.
        if zf >= ROUNDELS_FROM {
            let rr = lerp(zf, &[(10.5, 8.0), (12.0, 10.5), (16.0, 13.0)]);
            for (li, line) in sys.lines.iter().enumerate() {
                if line.badge.is_empty() {
                    continue;
                }
                let Some(longest) = line.paths.iter().max_by_key(|p| p.len()) else { continue };
                let ends = [longest.first().copied(), longest.last().copied()];
                for p in ends.into_iter().flatten() {
                    if let Some(s) = to_screen(p) {
                        if on_screen.contains(s) {
                            let c = line.color;
                            let a = alpha_of(li);
                            painter.circle(s, rr, fade(Color32::from_rgb(c[0], c[1], c[2]), a), Stroke::new(2.0_f32, fade(Color32::WHITE, a)));
                            painter.text(s, Align2::CENTER_CENTER, &line.badge, FontId::proportional(rr * 1.05), fade(Color32::WHITE, a));
                        }
                    }
                }
            }
        }

        // Trains (drawn after the station circles, below).
        if o.trains && train_a > 0.02 {
            for (li, line) in sys.lines.iter().enumerate() {
                if sel.map_or(false, |s| s != li) {
                    continue;
                }
                let len = line.track.length();
                if len <= 0.0 {
                    continue;
                }
                let n = train_count(len);
                let c = line.color;
                for k in 0..n {
                    let Some(p) = line.track.point_at(&line.paths, train_s(len, n, k, o.now)) else { continue };
                    if let Some(s) = to_screen(p) {
                        if on_screen.contains(s) {
                            trains.push((s, fade(Color32::from_rgb(c[0], c[1], c[2]), train_a)));
                        }
                    }
                }
            }
        }

        if zf >= STATIONS_FROM {
            for st in &sys.stations {
                if let Some(s) = to_screen(st.pos) {
                    if on_screen.contains(s) {
                        stations.push((s, st));
                    }
                }
            }
        }
    }
    out.hit = best_hit.map(|(_, a, i)| (a, i));

    // 4. Stations: a circle only where there is room for it, a name only where there is room to read it.
    if !stations.is_empty() {
        let pts: Vec<Pos2> = stations.iter().map(|s| s.0).collect();
        let gaps = nearest_gaps(&pts, MIN_NAME_GAP);
        let r_zoom = lerp(zf, &[(10.0, 3.6), (11.0, 5.6), (14.0, 8.0), (17.0, 10.5)]);
        for ((s, st), gap) in stations.iter().zip(gaps) {
            if gap < MIN_STATION_GAP {
                continue;
            }
            let inter = st.lines.len() >= 2;
            let base = r_zoom.min(gap * 0.36);
            let rr = if inter { base * 1.3 } else { base };
            let stroke = Stroke::new(if inter { 2.8_f32 } else { 2.0_f32 }, ink);
            match flat.then(|| flat_circle(&pr, rect, st.pos, rr, units_per_px)).flatten() {
                Some(pts) => {
                    painter.add(egui::Shape::convex_polygon(pts, Color32::WHITE, stroke));
                }
                None => {
                    painter.circle(*s, rr, Color32::WHITE, stroke);
                }
            }
            if !st.name.is_empty() && gap >= MIN_NAME_GAP {
                out.labels.push(StationLabel { at: *s + egui::vec2(0.0, -(rr + 10.0)), text: st.name.clone(), interchange: inter });
            }
        }
    }

    // 5. Trains on top: a white dot ringed in the line colour.
    if !trains.is_empty() {
        let r = (line_w * 0.55).clamp(3.5, 8.0);
        for (s, col) in trains {
            painter.circle(s, r, fade(Color32::WHITE, train_a), Stroke::new(2.4_f32, col));
        }
        out.animating = true;
    }
    out
}

/// The metro symbol under a screen position, if one is showing there.
pub fn marker_at(cam: &Camera, rect: egui::Rect, zf: f64, markers: &[Marker], at: Pos2) -> Option<usize> {
    let a = (1.0 - map_fade(zf)) * (((zf - 2.0) / 1.0).clamp(0.0, 1.0) as f32);
    if a < 0.3 {
        return None;
    }
    let pr = cam.projector(rect.width() as f64, rect.height() as f64);
    let r = marker_radius(zf) + 4.0;
    markers.iter().position(|m| {
        pr.visible(m.pos)
            && pr.project(m.pos).map_or(false, |s| (rect.min + egui::vec2(s[0] as f32, s[1] as f32)).distance(at) <= r)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_get_thicker_as_you_zoom_in() {
        assert!(line_width(9.0) <= 2.5, "slim when the whole city is in view");
        assert!(line_width(12.0) > line_width(9.5));
        assert!(line_width(17.0) >= 11.0);
    }

    #[test]
    fn map_fades_in_while_markers_fade_out() {
        assert_eq!(map_fade(MAP_FROM - 1.0), 0.0);
        assert_eq!(map_fade(MAP_FULL + 1.0), 1.0);
        let mid = map_fade((MAP_FROM + MAP_FULL) / 2.0);
        assert!(mid > 0.4 && mid < 0.6);
        assert!(STATIONS_FROM >= MAP_FULL);
    }

    #[test]
    fn crowded_stations_have_small_gaps_and_spread_ones_large() {
        let pts = [Pos2::new(0.0, 0.0), Pos2::new(10.0, 0.0), Pos2::new(500.0, 500.0), Pos2::new(560.0, 500.0)];
        let g = nearest_gaps(&pts, MIN_NAME_GAP);
        assert!((g[0] - 10.0).abs() < 1e-4 && (g[1] - 10.0).abs() < 1e-4);
        assert!(g[0] < MIN_STATION_GAP);
        assert_eq!(g[2], MIN_NAME_GAP, "60 px apart is beyond the cap");
        assert!(g[3] >= MIN_NAME_GAP);
    }

    #[test]
    fn flat_circle_is_round_from_above_and_squashed_when_tilted() {
        let mut cam = Camera::from_degrees(10.0, 40.0, 0.002);
        let (w, h) = (1000.0, 800.0);
        let rect = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(w as f32, h as f32));
        let upp = 2.0 * cam.alt * (FOV_Y * 0.5).tan() / h;
        let size = |cam: &Camera| {
            let pr = cam.projector(w, h);
            let pts = flat_circle(&pr, rect, cam.dir(), 20.0, upp).unwrap();
            let (mut lo, mut hi) = (pts[0], pts[0]);
            for p in &pts {
                lo = lo.min(*p);
                hi = hi.max(*p);
            }
            (hi.x - lo.x, hi.y - lo.y)
        };
        let (a, b) = size(&cam);
        assert!((a - 40.0).abs() < 1.5 && (b - 40.0).abs() < 1.5, "{a} x {b}");
        cam.pitch = 1.1;
        let (a, b) = size(&cam);
        assert!(b < a * 0.6 && (a - 40.0).abs() < 4.0, "{a} x {b}");
    }

    #[test]
    fn trains_shuttle_end_to_end_and_are_spread_out() {
        let len = 0.001; // about 6 km
        let n = train_count(len);
        assert!((2..=14).contains(&n));
        for step in 0..200 {
            for k in 0..n {
                let s = train_s(len, n, k, step as f64 * 7.0);
                assert!((0.0..=len).contains(&s), "{s}");
            }
        }
        let a = train_s(len, n, 0, 0.0);
        let b = train_s(len, n, 1, 0.0);
        assert!((a - b).abs() > 1e-6, "trains must not sit on top of each other");
    }

    #[test]
    fn clicking_near_a_line_finds_it() {
        let pts = [Pos2::new(0.0, 0.0), Pos2::new(100.0, 0.0), Pos2::new(100.0, 100.0)];
        assert!((dist_to_polyline(Pos2::new(50.0, 4.0), &pts) - 4.0).abs() < 1e-4);
        assert!((dist_to_polyline(Pos2::new(104.0, 50.0), &pts) - 4.0).abs() < 1e-4);
        assert!(dist_to_polyline(Pos2::new(50.0, 40.0), &pts) > 30.0);
    }
}
