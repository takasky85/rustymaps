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

use crate::camera::{dot, normalize, Camera, FOV_Y, V3};
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

/// Zoom levels from which each element appears.
pub const LINES_FROM: f64 = 5.5;
pub const ROUNDELS_FROM: f64 = 8.5;
pub const STATIONS_FROM: f64 = 8.0;

/// Line width in logical pixels at a zoom level (the white casing adds to it).
pub fn line_width(zf: f64) -> f32 {
    lerp(zf, &[(5.5, 3.0), (8.0, 4.6), (11.0, 7.0), (14.0, 10.0), (17.0, 13.5), (20.0, 17.0)])
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

pub fn draw(
    painter: &egui::Painter,
    rect: egui::Rect,
    cam: &Camera,
    zf: f64,
    systems: &[Arc<SystemData>],
) -> Vec<StationLabel> {
    let mut labels = Vec::new();
    if zf < LINES_FROM || systems.is_empty() {
        return labels;
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
    let casing = Color32::from_rgba_unmultiplied(255, 255, 255, 240);
    let casing_extra = (line_w * 0.28).clamp(2.0, 4.0);
    let ink = Color32::from_rgb(35, 35, 45);
    let mut stations: Vec<(Pos2, &transit_data::Station)> = Vec::new();

    for sys in systems {
        if !may_be_visible(cam, sys, w / h) {
            continue;
        }
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

        // 2. All white casings first, then all colours: crossings look like a drawn map.
        for (_, r) in &runs {
            painter.add(egui::Shape::line(r.clone(), Stroke::new(line_w + casing_extra, casing)));
        }
        for (li, r) in &runs {
            let c = sys.lines[*li].color;
            painter.add(egui::Shape::line(r.clone(), Stroke::new(line_w, Color32::from_rgb(c[0], c[1], c[2]))));
        }

        // 3. Roundels at the ends of each line's longest stretch.
        if zf >= ROUNDELS_FROM {
            let rr = lerp(zf, &[(8.5, 8.0), (12.0, 10.5), (16.0, 13.0)]);
            for line in &sys.lines {
                if line.badge.is_empty() {
                    continue;
                }
                let Some(longest) = line.paths.iter().max_by_key(|p| p.len()) else { continue };
                let ends = [longest.first().copied(), longest.last().copied()];
                for p in ends.into_iter().flatten() {
                    if let Some(s) = to_screen(p) {
                        if on_screen.contains(s) {
                            let c = line.color;
                            painter.circle(s, rr, Color32::from_rgb(c[0], c[1], c[2]), Stroke::new(2.0_f32, Color32::WHITE));
                            painter.text(s, Align2::CENTER_CENTER, &line.badge, FontId::proportional(rr * 1.05), Color32::WHITE);
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

    // 4. Stations: a circle only where there is room for it, a name only where there is room to read it.
    if !stations.is_empty() {
        let pts: Vec<Pos2> = stations.iter().map(|s| s.0).collect();
        let gaps = nearest_gaps(&pts, MIN_NAME_GAP);
        let r_zoom = lerp(zf, &[(8.0, 3.6), (11.0, 5.6), (14.0, 8.0), (17.0, 10.5)]);
        for ((s, st), gap) in stations.iter().zip(gaps) {
            if gap < MIN_STATION_GAP {
                continue;
            }
            let inter = st.lines.len() >= 2;
            let base = r_zoom.min(gap * 0.36);
            let rr = if inter { base * 1.3 } else { base };
            painter.circle(*s, rr, Color32::WHITE, Stroke::new(if inter { 2.8_f32 } else { 2.0_f32 }, ink));
            if !st.name.is_empty() && gap >= MIN_NAME_GAP {
                labels.push(StationLabel { at: *s + egui::vec2(0.0, -(rr + 10.0)), text: st.name.clone(), interchange: inter });
            }
        }
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_get_thicker_as_you_zoom_in() {
        assert!(line_width(6.0) >= 3.0);
        assert!(line_width(12.0) > line_width(8.0));
        assert!(line_width(17.0) >= 13.0);
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
}
