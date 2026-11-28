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
    /// Slot indices from the bottom of the drawing order to the top.
    order: Vec<usize>,
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
        let order: Vec<usize> = (0..slots.len()).collect();
        TransitManager { ctx: ctx.clone(), dir: data_dir, slots, order }
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
                let hit = (!st.name.is_empty() && st.name.to_lowercase().contains(&q)) || (!st.local.is_empty() && st.local.contains(query.trim()));
                let shown = if st.local.is_empty() || st.name.is_empty() { st.display().to_string() } else { format!("{} ({})", st.name, st.local) };
                if hit && !out.iter().any(|o| o.0 == shown && o.1 == cfg.title) {
                    out.push((shown, cfg.title, st.pos));
                }
            }
        }
        out
    }

    /// Slots of all enabled, loaded systems, in drawing order (the last is drawn on top).
    pub fn visible_slots(&self) -> Vec<usize> {
        self.order.iter().copied().filter(|&i| self.slots[i].enabled && self.slots[i].data.is_some()).collect()
    }

    /// Data of all enabled, loaded systems, in the same order as `visible_slots`.
    pub fn visible(&self) -> Vec<Arc<SystemData>> {
        self.visible_slots().into_iter().filter_map(|i| self.slots[i].data.clone()).collect()
    }

    /// Loaded systems (shown or hidden) whose area is around the camera, top of the drawing order first.
    pub fn nearby(&self, cam: &Camera, aspect: f64) -> Vec<usize> {
        self.order
            .iter()
            .rev()
            .copied()
            .filter(|&i| self.slots[i].data.as_ref().map_or(false, |d| may_be_visible(cam, d, aspect)))
            .collect()
    }

    /// Moves `slot` one place up (towards the top of the drawing order) or down, past the next
    /// system in `among` (the list the user is looking at).
    pub fn shift_priority(&mut self, slot: usize, up: bool, among: &[usize]) {
        let Some(at) = self.order.iter().position(|&i| i == slot) else { return };
        let other = if up {
            self.order[at + 1..].iter().position(|i| among.contains(i)).map(|k| at + 1 + k)
        } else {
            self.order[..at].iter().rposition(|i| among.contains(i))
        };
        if let Some(o) = other {
            self.order.swap(at, o);
        }
    }

    pub fn title(&self, slot: usize) -> &'static str {
        SYSTEMS[slot].title
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
pub fn fetch_to_dir(dir: &Path, only: &[String], max: usize) {
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
    let (mut tried, mut left_over) = (0usize, 0usize);
    // Systems that failed on an earlier night (they leave an `<id>.failed` note) go to the back of the
    // queue, so a difficult one cannot use up every night's batch.
    let mut queue: Vec<&transit_data::SystemConfig> =
        SYSTEMS.iter().filter(|c| only.is_empty() || only.iter().any(|o| o == c.id)).collect();
    // Saved files in the old format (English names only) are fetched again after the missing ones.
    let outdated = |id: &str| -> bool {
        use std::io::Read;
        let mut head = [0u8; 4];
        std::fs::File::open(dir.join(format!("{id}.tmap"))).and_then(|mut f| f.read_exact(&mut head)).is_ok()
            && transit_data::is_outdated(&head)
    };
    queue.sort_by_key(|c| {
        let missing = !dir.join(format!("{}.tmap", c.id)).is_file();
        (dir.join(format!("{}.failed", c.id)).is_file(), !missing)
    });
    for cfg in queue {
        let out = dir.join(format!("{}.tmap", cfg.id));
        if out.is_file() {
            if !outdated(cfg.id) {
                say(format!("{}: already saved, skipping", cfg.id));
                continue;
            }
            say(format!("{}: saved in the old format, fetching again", cfg.id));
        }
        // A batch size keeps one run short enough; the workflow starts the next batch by itself.
        if max > 0 && tried >= max {
            left_over += 1;
            continue;
        }
        tried += 1;
        let mut done = false;
        for attempt in 1..=3u64 {
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
                            let _ = std::fs::remove_file(dir.join(format!("{}.failed", cfg.id)));
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
            let _ = std::fs::write(dir.join(format!("{}.failed", cfg.id)), "");
            say(format!("{}: gave up for tonight", cfg.id));
        }
        std::thread::sleep(Duration::from_secs(20)); // be gentle between systems
    }
    say(format!("finished, {failed} system(s) missing, {left_over} left for the next night"));
    let _ = std::fs::write(dir.join("remaining.txt"), (failed + left_over).to_string());
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
    /// Second line (English under the local name), "" if none.
    pub sub: String,
    pub interchange: bool,
}

/// A station circle that was drawn this frame (for hover and click).
pub struct ShownStation {
    pub at: Pos2,
    pub r: f32,
    /// Index into the `systems` slice given to `draw`, and the station's index there.
    pub sys: usize,
    pub st: usize,
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

/// Size of a transit system's name on the globe: the size of a big city's label, or a little more.
const MARKER_LABEL_SIZE: f32 = 17.0;

fn marker_radius(zf: f64) -> f32 {
    lerp(zf, &[(2.0, 8.0), (5.0, 12.0), (9.0, 16.0)])
}

/// The text of a station's map label and the line to put under it: the local name with the English
/// one below when both exist and `bilingual`, otherwise just one name.
pub fn station_texts(st: &transit_data::Station, bilingual: bool) -> (String, String) {
    if bilingual && !st.local.is_empty() && !st.name.is_empty() && st.local != st.name {
        (st.local.clone(), st.name.clone())
    } else {
        (st.display().to_string(), String::new())
    }
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
        painter.circle_filled(c + egui::vec2(0.0, 2.0), r + 3.0, shadow);
        painter.circle_filled(c, r + 2.5, white);
        painter.circle_filled(c, r, blue);
        painter.text(c, Align2::CENTER_CENTER, "M", FontId::proportional(r * 1.3), white);
        if zf >= 5.0 {
            // black on a white halo, at least as big as a city name
            let font = FontId::new(MARKER_LABEL_SIZE, crate::fonts::Fonts::bold_family());
            let at = c + egui::vec2(0.0, r + 15.0);
            let halo = fade(Color32::WHITE, a);
            for (dx, dy) in [(-1.4_f32, 0.0_f32), (1.4, 0.0), (0.0, -1.4), (0.0, 1.4), (-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                painter.text(at + egui::vec2(dx, dy), Align2::CENTER_CENTER, m.title, font.clone(), halo);
            }
            painter.text(at, Align2::CENTER_CENTER, m.title, font, fade(Color32::BLACK, a));
        }
    }
}

/// Screen areas taken by the metro symbols (and their names), so labels can keep clear of them.
pub fn marker_rects(cam: &Camera, rect: egui::Rect, zf: f64, markers: &[Marker]) -> Vec<egui::Rect> {
    let a = (1.0 - map_fade(zf)) * (((zf - 2.0) / 1.0).clamp(0.0, 1.0) as f32);
    if a <= 0.01 {
        return Vec::new();
    }
    let pr = cam.projector(rect.width() as f64, rect.height() as f64);
    let r = marker_radius(zf) + 4.0;
    let mut out = Vec::new();
    for m in markers {
        if !pr.visible(m.pos) {
            continue;
        }
        let Some(s) = pr.project(m.pos) else { continue };
        let c = rect.min + egui::vec2(s[0] as f32, s[1] as f32);
        let mut r2 = egui::Rect::from_center_size(c, egui::vec2(2.0 * r, 2.0 * r));
        if zf >= 5.0 {
            let half = 4.7 * m.title.chars().count() as f32 + 4.0;
            r2 = r2.union(egui::Rect::from_center_size(c + egui::vec2(0.0, r + 15.0), egui::vec2(2.0 * half, 24.0)));
        }
        out.push(r2);
    }
    out
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
    // Size the circle by the less squashed direction, so it keeps its full width and only loses depth.
    let px_per_unit = s0.distance(se).max(s0.distance(sn)) as f64 / eps;
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
    /// 1 = real time; larger runs the trains faster.
    pub train_speed: f64,
    /// Where the user just clicked (screen position), to find which line was hit.
    pub pick: Option<Pos2>,
    /// The highlighted line: its system and its index there.
    pub selected: Option<(&'a Arc<SystemData>, usize)>,
    /// Show local names with English underneath where a station has both.
    pub bilingual: bool,
}

pub struct Out {
    pub labels: Vec<StationLabel>,
    /// The line under `pick`, if any.
    pub hit: Option<(Arc<SystemData>, usize)>,
    /// Trains are moving, so the caller should keep repainting.
    pub animating: bool,
    /// How fast the fastest train crosses the screen (pixels per second), to pick a redraw rate.
    pub motion_px_s: f32,
    /// The station circles drawn this frame.
    pub shown: Vec<ShownStation>,
}

/// Simulated trains: not a timetable, just evenly spread trains running out and back along each
/// line at real metro speeds, stopping at the stations.
const TRAIN_SPACING_KM: f64 = 2.5;
const TRAINS_FROM: f64 = 10.0;
use crate::transit_data::{EARTH_RADIUS_M, TRAIN_VMAX};

fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_sq();
    let t = if len2 <= 1e-9 { 0.0 } else { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) };
    p.distance(a + ab * t)
}

fn dist_to_polyline(p: Pos2, pts: &[Pos2]) -> f32 {
    pts.windows(2).map(|w| dist_to_segment(p, w[0], w[1])).fold(f32::INFINITY, f32::min)
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
    let mut out = Out { labels: Vec::new(), hit: None, animating: false, motion_px_s: 0.0, shown: Vec::new() };
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
    let mut stations: Vec<(Pos2, &transit_data::Station, usize, usize)> = Vec::new();
    let mut trains: Vec<(Pos2, Color32)> = Vec::new();
    let mut best_hit: Option<(f32, Arc<SystemData>, usize)> = None;
    let train_a = (((zf - TRAINS_FROM) / 1.0).clamp(0.0, 1.0) as f32) * fade_a;

    for (si, sys) in systems.iter().enumerate() {
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
                let cycle = line.track.cycle_seconds();
                for k in 0..n {
                    let t = o.now * o.train_speed + cycle * (k as f64 + 0.37) / n as f64;
                    let Some(p) = line.track.point_at(&line.paths, line.track.s_at(t)) else { continue };
                    if let Some(s) = to_screen(p) {
                        if on_screen.contains(s) {
                            trains.push((s, fade(Color32::from_rgb(c[0], c[1], c[2]), train_a)));
                        }
                    }
                }
            }
        }

        if zf >= STATIONS_FROM {
            for (sti, st) in sys.stations.iter().enumerate() {
                if let Some(s) = to_screen(st.pos) {
                    if on_screen.contains(s) {
                        stations.push((s, st, si, sti));
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
        for ((s, st, si, sti), gap) in stations.iter().zip(gaps) {
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
            out.shown.push(ShownStation { at: *s, r: rr, sys: *si, st: *sti });
            if gap >= MIN_NAME_GAP {
                let (text, sub) = station_texts(st, o.bilingual);
                if !text.is_empty() {
                    out.labels.push(StationLabel { at: *s + egui::vec2(0.0, -(rr + if sub.is_empty() { 10.0 } else { 16.0 })), text, sub, interchange: inter });
                }
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
        let metres_per_px = 2.0 * std::f64::consts::PI * EARTH_RADIUS_M * cam.lat.cos().max(0.05) / (512.0 * 2f64.powf(zf));
        out.motion_px_s = (TRAIN_VMAX * o.train_speed / metres_per_px) as f32;
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

// ---------------------------------------------------------------------------
// Platform sign
// ---------------------------------------------------------------------------

/// The stations on either side of `st` along `line`: [left, right] (west before east on screen,
/// north up). The nearest neighbour comes first; the second is the nearest one lying the other way.
pub fn sign_neighbours(sys: &SystemData, st: usize, line: usize) -> [Option<usize>; 2] {
    let Some(me) = sys.stations.get(st) else { return [None, None] };
    let rel = |s: usize| -> V3 {
        let p = sys.stations[s].pos;
        [p[0] - me.pos[0], p[1] - me.pos[1], p[2] - me.pos[2]]
    };
    let len2 = |v: V3| v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
    let mut near: Vec<usize> =
        me.nbrs.iter().filter(|(l, s)| *l as usize == line && (*s as usize) < sys.stations.len() && *s as usize != st).map(|&(_, s)| s as usize).collect();
    near.sort_by(|&a, &b| len2(rel(a)).partial_cmp(&len2(rel(b))).unwrap_or(std::cmp::Ordering::Equal));
    near.dedup();
    let Some(&a) = near.first() else { return [None, None] };
    let b = near.iter().skip(1).copied().find(|&s| dot(rel(s), rel(a)) < 0.0);
    // Which one is "left": the one further west (smaller longitude), else the southern one.
    let key = |s: usize| {
        let (lon, lat) = crate::camera::vec_to_lonlat(sys.stations[s].pos);
        (lon, lat)
    };
    match b {
        Some(b) => {
            let (ka, kb) = (key(a), key(b));
            if (ka.0 - kb.0).abs() > 1e-9 && ka.0 < kb.0 || ((ka.0 - kb.0).abs() <= 1e-9 && ka.1 < kb.1) {
                [Some(a), Some(b)]
            } else {
                [Some(b), Some(a)]
            }
        }
        None => {
            let (km, ka) = (crate::camera::vec_to_lonlat(me.pos), key(a));
            if ka.0 < km.0 {
                [Some(a), None]
            } else {
                [None, Some(a)]
            }
        }
    }
}

/// Text laid out at `size`, shrunk to fit `max_w` if it is too wide.
fn fit_text(painter: &egui::Painter, text: &str, size: f32, bold: bool, color: Color32, max_w: f32) -> Arc<egui::Galley> {
    let family = if bold { crate::fonts::Fonts::bold_family() } else { crate::fonts::Fonts::family() };
    let g = painter.layout_no_wrap(text.to_string(), FontId::new(size, family.clone()), color);
    let w = g.size().x;
    if w <= max_w || w <= 0.0 {
        return g;
    }
    painter.layout_no_wrap(text.to_string(), FontId::new((size * max_w / w).max(7.0), family), color)
}

pub const SIGN_W: f32 = 304.0;
pub const SIGN_H: f32 = 142.0;

/// A station sign like the ones on a Japanese platform: the name big, its reading above or below,
/// a bar in the line's colour with the neighbouring stations at either end and this station in the
/// middle, and the English name underneath. `anchor` is the point the sign's bottom edge points at.
pub fn draw_sign(painter: &egui::Painter, clip: egui::Rect, anchor: Pos2, sys: &SystemData, st_i: usize, line_pick: usize) {
    let Some(st) = sys.stations.get(st_i) else { return };
    let line_i = if st.lines.is_empty() { None } else { Some(st.lines[line_pick % st.lines.len()]) };
    let line = line_i.and_then(|i| sys.lines.get(i));
    let lc = line.map_or([60, 120, 200], |l| l.color);
    let line_col = Color32::from_rgb(lc[0], lc[1], lc[2]);
    let block_col = Color32::from_rgb(
        (lc[0] as u16 * 45 / 100 + 255 * 55 / 100) as u8,
        (lc[1] as u16 * 45 / 100 + 255 * 55 / 100) as u8,
        (lc[2] as u16 * 45 / 100 + 255 * 55 / 100) as u8,
    );
    let ink = Color32::from_rgb(28, 28, 32);

    let mut min = anchor + egui::vec2(-SIGN_W * 0.5, -SIGN_H - 14.0);
    min.x = min.x.clamp(clip.min.x + 4.0, (clip.max.x - SIGN_W - 4.0).max(clip.min.x + 4.0));
    min.y = min.y.max(clip.min.y + 4.0);
    let r = egui::Rect::from_min_size(min, egui::vec2(SIGN_W, SIGN_H));
    painter.rect_filled(r.translate(egui::vec2(0.0, 3.0)).expand(1.0), 9.0, Color32::from_black_alpha(80));
    painter.rect_filled(r, 8.0, Color32::from_rgb(252, 252, 250));
    painter.rect_stroke(r, 8.0, Stroke::new(4.0_f32, ink), egui::StrokeKind::Inside);

    let cx = r.center().x;
    // the name: local script big, with its reading; otherwise the English name big
    let (big, small) = if st.local.is_empty() { (st.name.as_str(), "") } else { (st.local.as_str(), st.name.as_str()) };
    let g = fit_text(painter, big, 34.0, true, ink, SIGN_W - 96.0);
    painter.galley(egui::pos2(cx - g.size().x * 0.5, r.min.y + 30.0 - g.size().y * 0.5), g, ink);
    let under = if !st.kana.is_empty() {
        st.kana.as_str()
    } else {
        ""
    };
    if !under.is_empty() {
        let g = fit_text(painter, under, 12.5, false, Color32::from_rgb(70, 70, 75), SIGN_W - 120.0);
        painter.galley(egui::pos2(cx - g.size().x * 0.5, r.min.y + 54.0), g, ink);
    }

    // line code box (top right): "JY03", else the line's badge
    let code = if !st.code.is_empty() { st.code.as_str() } else { line.map_or("", |l| l.badge.as_str()) };
    if !code.is_empty() {
        let g = fit_text(painter, code, 12.0, true, ink, 44.0);
        let b = egui::Rect::from_center_size(egui::pos2(r.max.x - 32.0, r.min.y + 24.0), egui::vec2(g.size().x + 12.0, 22.0));
        painter.rect_stroke(b, 3.0, Stroke::new(2.0_f32, ink), egui::StrokeKind::Inside);
        painter.galley(b.center() - g.size() * 0.5, g, ink);
    }

    // the bar
    let [left, right] = sign_neighbours(sys, st_i, line_i.unwrap_or(usize::MAX));
    let bar_y0 = r.min.y + 76.0;
    let bar_y1 = bar_y0 + 28.0;
    let (x0, x1) = (r.min.x + 4.0, r.max.x - 4.0);
    let mid_y = (bar_y0 + bar_y1) * 0.5;
    painter.add(egui::Shape::convex_polygon(
        vec![egui::pos2(x0, bar_y0), egui::pos2(x1 - 16.0, bar_y0), egui::pos2(x1, mid_y), egui::pos2(x1 - 16.0, bar_y1), egui::pos2(x0, bar_y1)],
        line_col,
        Stroke::NONE,
    ));
    let block_w = 22.0;
    painter.rect_filled(egui::Rect::from_min_max(egui::pos2(cx - block_w * 0.5, bar_y0), egui::pos2(cx + block_w * 0.5, bar_y1)), 0.0, block_col);
    let side_w = (SIGN_W * 0.5 - block_w * 0.5 - 18.0).max(40.0);
    let white = Color32::WHITE;
    let pick_names = |n: Option<usize>| -> (String, String) {
        match n.and_then(|i| sys.stations.get(i)) {
            Some(s) if !s.local.is_empty() => (s.local.clone(), s.name.clone()),
            Some(s) => (s.name.clone(), String::new()),
            None => (String::new(), String::new()),
        }
    };
    let (lj, le) = pick_names(left);
    let (rj, re) = pick_names(right);
    if !lj.is_empty() {
        let g = fit_text(painter, &lj, 13.5, true, white, side_w);
        painter.galley(egui::pos2(x0 + 8.0, mid_y - g.size().y * 0.5), g, white);
    }
    if !rj.is_empty() {
        let g = fit_text(painter, &rj, 13.5, true, white, side_w);
        painter.galley(egui::pos2(x1 - 22.0 - g.size().x, mid_y - g.size().y * 0.5), g, white);
    }
    // English under the bar: neighbours small at the ends, this station in the middle
    let grey = Color32::from_rgb(50, 50, 55);
    let en_y = bar_y1 + 7.0;
    if !le.is_empty() {
        let g = fit_text(painter, &le, 10.5, false, grey, side_w);
        painter.galley(egui::pos2(x0 + 8.0, en_y), g, grey);
    }
    if !re.is_empty() {
        let g = fit_text(painter, &re, 10.5, false, grey, side_w);
        painter.galley(egui::pos2(x1 - 22.0 - g.size().x, en_y), g, grey);
    }
    if !small.is_empty() {
        let g = fit_text(painter, small, 15.0, true, ink, block_w + 40.0);
        painter.galley(egui::pos2(cx - g.size().x * 0.5, en_y - 1.0), g, ink);
    }
    // more than one line serves this station: little dots, the shown one filled
    if st.lines.len() > 1 {
        let n = st.lines.len().min(8);
        let y = r.max.y - 10.0;
        let x_start = cx - (n as f32 - 1.0) * 7.0;
        for k in 0..n {
            let c = sys.lines.get(st.lines[k]).map_or([120, 120, 120], |l| l.color);
            let col = Color32::from_rgb(c[0], c[1], c[2]);
            let at = egui::pos2(x_start + k as f32 * 14.0, y);
            if k == line_pick % st.lines.len() {
                painter.circle(at, 4.5, col, Stroke::new(1.5_f32, ink));
            } else {
                painter.circle_stroke(at, 3.5, Stroke::new(1.5_f32, col));
            }
        }
    }
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
    fn a_sensible_number_of_trains_per_line() {
        for len_km in [1.0, 6.0, 30.0, 200.0] {
            let n = train_count(len_km * 1000.0 / EARTH_RADIUS_M);
            assert!((2..=14).contains(&n), "{n}");
        }
    }

    #[test]
    fn clicking_near_a_line_finds_it() {
        let pts = [Pos2::new(0.0, 0.0), Pos2::new(100.0, 0.0), Pos2::new(100.0, 100.0)];
        assert!((dist_to_polyline(Pos2::new(50.0, 4.0), &pts) - 4.0).abs() < 1e-4);
        assert!((dist_to_polyline(Pos2::new(104.0, 50.0), &pts) - 4.0).abs() < 1e-4);
        assert!(dist_to_polyline(Pos2::new(50.0, 40.0), &pts) > 30.0);
    }

    #[test]
    fn the_sign_picks_the_two_neighbours_on_opposite_sides() {
        use crate::transit_data::{Station, TLine, Track};
        use crate::camera::lonlat_to_vec;
        let st = |name: &str, lon: f64| Station {
            name: name.into(),
            local: String::new(),
            kana: String::new(),
            code: String::new(),
            pos: lonlat_to_vec(lon.to_radians(), 0.6),
            lines: vec![0],
            nbrs: Vec::new(),
        };
        let mut d = SystemData::default();
        d.lines.push(TLine { name: "L".into(), local: String::new(), badge: String::new(), color: [0, 150, 0], paths: Vec::new(), track: Track::default() });
        d.stations = vec![st("west", 0.0), st("mid", 0.01), st("east", 0.02), st("far east", 0.05)];
        // mid is next to west, east and (as an express) far east
        d.stations[1].nbrs = vec![(0, 3), (0, 2), (0, 0)];
        let [l, r] = sign_neighbours(&d, 1, 0);
        assert_eq!((l, r), (Some(0), Some(2)), "nearest each way, west on the left");
        // an end of the line has one neighbour, on the side it lies
        d.stations[0].nbrs = vec![(0, 1)];
        assert_eq!(sign_neighbours(&d, 0, 0), [None, Some(1)]);
        assert_eq!(sign_neighbours(&d, 0, 7), [None, None], "another line: none");
    }

    #[test]
    fn station_labels_show_local_over_english_when_asked() {
        use crate::transit_data::Station;
        let s = Station {
            name: "Akihabara".into(),
            local: "秋葉原".into(),
            kana: String::new(),
            code: String::new(),
            pos: [0.0, 0.0, 1.0],
            lines: vec![],
            nbrs: vec![],
        };
        assert_eq!(station_texts(&s, true), ("秋葉原".to_string(), "Akihabara".to_string()));
        assert_eq!(station_texts(&s, false), ("Akihabara".to_string(), String::new()));
        let only_local = Station { name: String::new(), ..s };
        assert_eq!(station_texts(&only_local, false).0, "秋葉原");
    }
}
