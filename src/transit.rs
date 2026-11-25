//! Transit overlays: downloads route geometry from OpenStreetMap (Overpass) once, caches it on
//! disk, and draws it in a subway-map style (thick colour-coded lines, white-and-black station
//! circles, bigger rings at interchanges, lettered roundels at line ends) on top of the map.
//! The lines follow the real tracks; only the styling is "subway map".

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Stroke};

use crate::camera::{dot, Camera, V3};
use crate::tiles::USER_AGENT;
use crate::transit_data::{self, SystemData, SYSTEMS};

const OVERPASS_URL: &str = "https://overpass-api.de/api/interpreter";
const CACHE_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);

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
}

pub struct TransitManager {
    ctx: egui::Context,
    cache_dir: PathBuf,
    slots: Vec<Slot>,
}

impl TransitManager {
    pub fn new(ctx: &egui::Context, cache_dir: PathBuf) -> Self {
        let slots = SYSTEMS
            .iter()
            .map(|_| Slot { enabled: false, status: Status::Off, data: None, pending: Arc::new(Mutex::new(None)) })
            .collect();
        TransitManager { ctx: ctx.clone(), cache_dir, slots }
    }

    pub fn enabled(&self, i: usize) -> bool {
        self.slots[i].enabled
    }

    pub fn status(&self, i: usize) -> Status {
        self.slots[i].status.clone()
    }

    pub fn set_enabled(&mut self, i: usize, on: bool) {
        let slot = &mut self.slots[i];
        slot.enabled = on;
        if on && slot.data.is_none() && slot.status != Status::Loading {
            slot.status = Status::Loading;
            let pending = slot.pending.clone();
            let ctx = self.ctx.clone();
            let dir = self.cache_dir.clone();
            let _ = std::thread::Builder::new().name(format!("transit-{}", SYSTEMS[i].id)).spawn(move || {
                let res = load(i, &dir);
                *pending.lock().unwrap() = Some(res);
                ctx.request_repaint();
            });
        }
    }

    /// Picks up finished downloads; call once per frame.
    pub fn poll(&mut self) {
        for slot in &mut self.slots {
            let done = slot.pending.lock().unwrap().take();
            match done {
                Some(Ok(d)) => {
                    slot.data = Some(Arc::new(d));
                    slot.status = Status::Ready;
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

fn load(i: usize, dir: &PathBuf) -> Result<SystemData, String> {
    let cfg = &SYSTEMS[i];
    let path = dir.join(format!("{}.json", cfg.id));
    let fresh = std::fs::metadata(&path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .map_or(false, |age| age < CACHE_MAX_AGE);
    let cached = if fresh { std::fs::read_to_string(&path).ok() } else { None };
    let (text, from_cache) = match cached {
        Some(t) => (t, true),
        None => (fetch(&transit_data::overpass_query(cfg))?, false),
    };
    match transit_data::parse(cfg, &text) {
        Ok(d) if !d.lines.is_empty() => {
            if !from_cache {
                let _ = std::fs::create_dir_all(dir);
                let _ = std::fs::write(&path, &text);
            }
            Ok(d)
        }
        Ok(_) => {
            let _ = std::fs::remove_file(&path);
            Err(format!("OpenStreetMap returned no {} lines (tagging may differ from what the query expects)", cfg.title))
        }
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            Err(format!("could not read the {} data: {e}", cfg.title))
        }
    }
}

fn fetch(query: &str) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(150)))
        .user_agent(USER_AGENT)
        .http_status_as_error(false)
        .build()
        .into();
    let url = format!("{OVERPASS_URL}?data={}", transit_data::url_encode(query));
    let mut resp = agent.get(&url).call().map_err(|e| format!("network error: {e}"))?;
    let status = resp.status().as_u16();
    if status != 200 {
        return Err(format!("Overpass answered HTTP {status}; try again in a minute"));
    }
    resp.body_mut().with_config().limit(200 * 1024 * 1024).read_to_string().map_err(|e| e.to_string())
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

pub fn draw(
    painter: &egui::Painter,
    rect: egui::Rect,
    cam: &Camera,
    zf: f64,
    systems: &[Arc<SystemData>],
) -> Vec<StationLabel> {
    let mut labels = Vec::new();
    if zf < 8.0 || systems.is_empty() {
        return labels;
    }
    let (w, h) = (rect.width() as f64, rect.height() as f64);
    let cpos = cam.pos();
    let visible = |p: V3| dot(p, cpos) > 1.0 + 1e-9;
    let to_screen = |p: V3| -> Option<Pos2> {
        if !visible(p) {
            return None;
        }
        cam.project(p, w, h).map(|s| rect.min + egui::vec2(s[0] as f32, s[1] as f32))
    };
    let on_screen = rect.expand(80.0);

    let line_w = lerp(zf, &[(8.0, 2.2), (12.0, 4.5), (16.0, 8.5)]);
    let casing = Color32::from_rgba_unmultiplied(255, 255, 255, 235);
    let ink = Color32::from_rgb(35, 35, 45);

    for sys in systems {
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
            painter.add(egui::Shape::line(r.clone(), Stroke::new(line_w + 3.0, casing)));
        }
        for (li, r) in &runs {
            let c = sys.lines[*li].color;
            painter.add(egui::Shape::line(r.clone(), Stroke::new(line_w, Color32::from_rgb(c[0], c[1], c[2]))));
        }

        // 3. Roundels at the ends of each line's longest stretch.
        if zf >= 10.5 {
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
                            painter.circle(s, 9.5, Color32::from_rgb(c[0], c[1], c[2]), Stroke::new(2.0_f32, Color32::WHITE));
                            painter.text(s, Align2::CENTER_CENTER, &line.badge, FontId::proportional(11.0), Color32::WHITE);
                        }
                    }
                }
            }
        }

        // 4. Stations.
        if zf >= 10.5 {
            let r = lerp(zf, &[(10.5, 3.0), (14.0, 5.5), (17.0, 8.0)]);
            for st in &sys.stations {
                let Some(s) = to_screen(st.pos) else { continue };
                if !on_screen.contains(s) {
                    continue;
                }
                let inter = st.lines.len() >= 2;
                let rr = if inter { r * 1.35 } else { r };
                painter.circle(s, rr, Color32::WHITE, Stroke::new(if inter { 2.6_f32 } else { 1.8_f32 }, ink));
                if !st.name.is_empty() && zf >= 12.2 {
                    labels.push(StationLabel { at: s + egui::vec2(0.0, -(rr + 9.0)), text: st.name.clone(), interchange: inter });
                }
            }
        }
    }
    labels
}
