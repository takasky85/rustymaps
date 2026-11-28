//! GPS tracks (KML files from the phone tracker) kept in `data/tracks/` and drawn over the map.
//! Files are read on a worker thread; the folder is the whole database, so copying a `.kml` in
//! (or, later, the phone sending one) is all it takes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use eframe::egui::{self, Color32, Pos2, Stroke};

use crate::camera::{vec_to_lonlat, Camera, V3};
use crate::clean;
use crate::kml;
use crate::replay::Timeline;
use crate::snap_tiles::{self, SnappedTrack};
use crate::tiles::DiskCache;
use crate::transit::circle_may_be_visible;

/// Which version of the paths is drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    /// Exactly what the phone recorded.
    Original,
    /// The tidied copy (spikes removed, smoothed).
    Cleaned,
    /// The original, thin and faint, under the tidied one.
    Both,
    /// The tidied copy pulled onto the streets (see `snap`); the original is drawn faint beneath it.
    Snapped,
}

impl View {
    pub const ALL: [(View, &'static str); 4] = [(View::Original, "Original"), (View::Cleaned, "Cleaned up"), (View::Both, "Both"), (View::Snapped, "Snapped to streets")];

    pub fn key(self) -> &'static str {
        match self {
            View::Original => "original",
            View::Cleaned => "cleaned",
            View::Both => "both",
            View::Snapped => "snapped",
        }
    }

    pub fn from_key(k: &str) -> Option<View> {
        View::ALL.iter().map(|v| v.0).find(|v| v.key() == k)
    }

    /// True when the tidied copy is what gets replayed and measured.
    pub fn uses_clean(self) -> bool {
        self != View::Original
    }
}

/// The track that is selected (clicked), with its replay.
pub struct Player {
    /// Index into `TrackSet::items`.
    pub item: usize,
    /// Seconds of walking since the start.
    pub u: f64,
    pub playing: bool,
    /// Replay speed: seconds of walk per second of watching.
    pub mult: f64,
    /// Keep the map centred on the walker while playing.
    pub follow: bool,
}

pub const REPLAY_SPEEDS: [f64; 5] = [1.0, 10.0, 30.0, 100.0, 400.0];

impl Player {
    /// A new selection; the replay speed starts at the one that shows the whole walk in about 40 s.
    pub fn new(item: usize, total_secs: f64) -> Player {
        let want = total_secs / 40.0;
        let mult = REPLAY_SPEEDS.iter().copied().min_by(|a, b| (a.ln() - want.max(1.0).ln()).abs().partial_cmp(&(b.ln() - want.max(1.0).ln()).abs()).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(10.0);
        Player { item, u: 0.0, playing: false, mult, follow: true }
    }

    /// Moves the replay on by `dt` real seconds; stops at the end.
    pub fn advance(&mut self, dt: f64, total_secs: f64) {
        if self.playing {
            self.u += dt * self.mult;
            if self.u >= total_secs {
                self.u = total_secs;
                self.playing = false;
            }
        }
    }
}

pub struct Track {
    pub name: String,
    /// File name inside the tracks folder.
    pub file: String,
    pub segs: Vec<Vec<V3>>,
    /// The same stretches, tidied (see `clean`).
    pub clean_segs: Vec<Vec<V3>>,
    /// Clocks of the original and the tidied points.
    pub orig: Timeline,
    pub cleaned: Timeline,
    /// The tidied points' times (one list per stretch, empty where the file had none).
    pub clean_times: Vec<Vec<f64>>,
    /// How long the track lasted, when only its name says.
    pub span: Option<f64>,
    /// Filled in by a background thread the first time the snapped view is wanted.
    pub snapped: OnceLock<SnappedTrack>,
    pub center: V3,
    /// Angular radius (radians) around `center` that holds every point.
    pub radius: f64,
    pub km: f64,
    pub color: [u8; 3],
}

impl Track {
    /// The clock of the version being shown.
    pub fn timeline(&self, view: View) -> &Timeline {
        match view {
            View::Original => &self.orig,
            View::Snapped => self.snapped.get().map_or(&self.cleaned, |s| &s.timeline),
            _ => &self.cleaned,
        }
    }

    /// The stretches that are drawn as the main line in `view`.
    pub fn main_segs(&self, view: View) -> &[Vec<V3>] {
        match view {
            View::Original => &self.segs,
            View::Snapped => self.snapped.get().map_or(&self.clean_segs[..], |s| &s.segs[..]),
            _ => &self.clean_segs,
        }
    }

    /// Longitude and latitude (degrees) of the middle of the track.
    pub fn lon_lat_deg(&self) -> (f64, f64) {
        let (lon, lat) = vec_to_lonlat(self.center);
        (lon.to_degrees(), lat.to_degrees())
    }
}

pub struct Item {
    pub track: Arc<Track>,
    pub shown: bool,
}

pub struct TrackSet {
    ctx: egui::Context,
    dir: PathBuf,
    pub items: Vec<Item>,
    incoming: Arc<Mutex<Vec<Track>>>,
    cache: Arc<DiskCache>,
    /// True while a background thread is snapping tracks.
    snapping: Arc<AtomicBool>,
    /// How strongly snapping prefers main streets over alleys and footpaths (0 to 1).
    pub snap_prefer: f32,
    /// A line about the last import, for the Layers panel.
    pub message: String,
}

const COLORS: [[u8; 3]; 6] = [[255, 87, 34], [0, 176, 255], [233, 30, 99], [255, 193, 7], [0, 200, 83], [170, 0, 255]];

fn build(p: kml::Parsed, file: &str) -> Track {
    let mut sum = [0.0; 3];
    let mut n = 0.0;
    for s in &p.segs {
        for q in s {
            for k in 0..3 {
                sum[k] += q[k];
            }
            n += 1.0;
        }
    }
    let center = crate::camera::normalize([sum[0] / n, sum[1] / n, sum[2] / n]);
    let radius = p.segs.iter().flatten().map(|q| crate::camera::dot(center, *q).clamp(-1.0, 1.0).acos()).fold(0.0, f64::max).max(1e-6);
    let km = p.segs.iter().map(|s| kml::length_km(s)).sum();
    let mut clean_segs = Vec::new();
    let mut clean_times = Vec::new();
    for (i, s) in p.segs.iter().enumerate() {
        let (c, t) = clean::clean(s, p.times.get(i).map_or(&[][..], |t| &t[..]));
        clean_segs.push(c);
        clean_times.push(t);
    }
    let span = kml::name_span_secs(&p.name);
    let orig = Timeline::new(&p.segs, &p.times, span);
    let cleaned = Timeline::new(&clean_segs, &clean_times, span);
    Track { name: p.name, file: file.to_string(), segs: p.segs, clean_segs, orig, cleaned, clean_times, span, snapped: OnceLock::new(), center, radius, km, color: [0; 3] }
}

/// Reads one KML file into tracks (empty when it holds none).
fn read_file(path: &Path) -> Vec<Track> {
    let Ok(bytes) = std::fs::read(path) else { return Vec::new() };
    let text = String::from_utf8_lossy(&bytes);
    let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let stem = path.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Track".into());
    kml::parse(&text, &stem).into_iter().map(|p| build(p, &file)).collect()
}

fn is_kml(p: &Path) -> bool {
    p.extension().map_or(false, |e| e.to_string_lossy().eq_ignore_ascii_case("kml"))
}

/// Copies the `.kml` files of an older tracks folder that are not in `to` yet.
fn adopt_old_files(from: &Path, to: &Path) {
    if from == to {
        return;
    }
    let Ok(rd) = std::fs::read_dir(from) else { return };
    for e in rd.filter_map(|e| e.ok()) {
        let p = e.path();
        if is_kml(&p) {
            if let Some(n) = p.file_name() {
                let dest = to.join(n);
                if !dest.exists() {
                    let _ = std::fs::copy(&p, &dest);
                }
            }
        }
    }
}

impl TrackSet {
    /// `data_dir` is the permanent per-user folder. `legacy_dir` is the `data` folder next to the
    /// program (every new build is unzipped with its own), whose `tracks` older versions used:
    /// anything found there is copied over once.
    pub fn new(ctx: &egui::Context, data_dir: &Path, legacy_dir: &Path) -> TrackSet {
        let dir = data_dir.join("tracks");
        let _ = std::fs::create_dir_all(&dir);
        adopt_old_files(&legacy_dir.join("tracks"), &dir);
        let set = TrackSet { ctx: ctx.clone(), dir: dir.clone(), items: Vec::new(), incoming: Arc::new(Mutex::new(Vec::new())), cache: Arc::new(DiskCache::new()), snapping: Arc::new(AtomicBool::new(false)), snap_prefer: 1.0, message: String::new() };
        let incoming = set.incoming.clone();
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new().name("tracks-load".into()).spawn(move || {
            let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| is_kml(p)).collect()).unwrap_or_default();
            files.sort();
            for f in files {
                let t = read_file(&f);
                if !t.is_empty() {
                    incoming.lock().unwrap().extend(t);
                    ctx.request_repaint();
                }
            }
        });
        set
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Starts snapping the tracks that have not been (one background thread, one track at a time).
    /// Call every frame while the snapped view is on; it does nothing when there is nothing to do.
    pub fn request_snap(&self) {
        let todo: Vec<Arc<Track>> = self.items.iter().filter(|i| i.track.snapped.get().is_none()).map(|i| i.track.clone()).collect();
        if todo.is_empty() || self.snapping.swap(true, Ordering::AcqRel) {
            return;
        }
        let (cache, flag, ctx, prefer) = (self.cache.clone(), self.snapping.clone(), self.ctx.clone(), self.snap_prefer as f64);
        let spawned = std::thread::Builder::new().name("tracks-snap".into()).spawn({
            let flag = flag.clone();
            move || {
                for t in todo {
                    let r = snap_tiles::snap_track(&t.clean_segs, &t.clean_times, t.span, prefer, &cache);
                    let _ = t.snapped.set(r);
                    ctx.request_repaint();
                }
                flag.store(false, Ordering::Release);
            }
        });
        if spawned.is_err() {
            flag.store(false, Ordering::Release);
        }
    }

    /// A line for the Layers panel about the snapping: progress, and what could not be done.
    pub fn snap_status(&self) -> String {
        let waiting = self.items.iter().filter(|i| i.track.snapped.get().is_none()).count();
        if waiting > 0 {
            return format!("Snapping {waiting} track(s) to the streets...");
        }
        let (mut matched, mut total, mut missing) = (0, 0, 0);
        for i in &self.items {
            if let Some(s) = i.track.snapped.get() {
                matched += s.matched;
                total += s.total;
                missing += s.missing_tiles;
            }
        }
        if total == 0 {
            return String::new();
        }
        let mut line = format!("{}% of the points are on a street.", matched * 100 / total);
        if missing > 0 {
            line.push_str(&format!(" {missing} map tile(s) were not on disk: look at those places on the map once, then press Redo."));
        }
        line
    }

    /// True when some track lacked map tiles when it was snapped.
    pub fn snap_incomplete(&self) -> bool {
        self.items.iter().any(|i| i.track.snapped.get().map_or(false, |s| s.missing_tiles > 0))
    }

    /// Changes how strongly main streets are preferred and snaps again the tracks that were done
    /// with the old value (the others will use the new one when their turn comes).
    pub fn set_snap_prefer(&mut self, v: f32) {
        let v = v.clamp(0.0, 1.0);
        if (v - self.snap_prefer).abs() < 1e-4 {
            return;
        }
        self.snap_prefer = v;
        let files: Vec<String> = self.items.iter().filter(|i| i.track.snapped.get().is_some()).map(|i| i.track.file.clone()).collect();
        for f in files {
            self.reload_file(&f);
        }
    }

    /// Snaps again the tracks that were short of map tiles (after the tiles have been viewed).
    pub fn resnap(&mut self) {
        let files: Vec<String> = self.items.iter().filter(|i| i.track.snapped.get().map_or(false, |s| s.missing_tiles > 0)).map(|i| i.track.file.clone()).collect();
        for f in files {
            self.reload_file(&f);
        }
    }

    /// Picks up tracks the worker has read; call once per frame.
    pub fn poll(&mut self) {
        let mut new: Vec<Track> = std::mem::take(&mut *self.incoming.lock().unwrap());
        // a file the phone has just sent is already here in its newer form
        let have: std::collections::HashSet<String> = self.items.iter().map(|i| i.track.file.clone()).collect();
        new.retain(|t| !have.contains(&t.file));
        for mut t in new {
            t.color = COLORS[self.items.len() % COLORS.len()];
            self.items.push(Item { track: Arc::new(t), shown: true });
        }
    }

    /// Reads a file in the tracks folder again (the phone sent a newer copy); its tracks replace the
    /// old ones in place, keeping their colour and whether they were shown.
    pub fn reload_file(&mut self, file: &str) {
        let fresh = read_file(&self.dir.join(file));
        let at = self.items.iter().position(|i| i.track.file == file);
        let keep = at.map(|k| (self.items[k].track.color, self.items[k].shown));
        self.items.retain(|i| i.track.file != file);
        let mut at = at.unwrap_or(self.items.len()).min(self.items.len());
        for mut t in fresh {
            t.color = keep.map_or(COLORS[self.items.len() % COLORS.len()], |k| k.0);
            self.items.insert(at, Item { track: Arc::new(t), shown: keep.map_or(true, |k| k.1) });
            at += 1;
        }
        self.message = format!("Updated {file}");
    }

    /// Copies a dropped `.kml` into the tracks folder and reads it.
    pub fn import(&mut self, path: &Path) {
        if !is_kml(path) {
            self.message = format!("{} is not a .kml file", path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
            return;
        }
        let stem = path.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "track".into());
        let mut dest = self.dir.join(format!("{stem}.kml"));
        let mut n = 2;
        while dest.exists() {
            dest = self.dir.join(format!("{stem}-{n}.kml"));
            n += 1;
        }
        if let Err(e) = std::fs::copy(path, &dest) {
            self.message = format!("Could not copy the file: {e}");
            return;
        }
        let tracks = read_file(&dest);
        if tracks.is_empty() {
            let _ = std::fs::remove_file(&dest);
            self.message = "No tracks found in that file".to_string();
            return;
        }
        self.message = format!("Added {} track(s) from {}", tracks.len(), stem);
        self.incoming.lock().unwrap().extend(tracks);
        self.ctx.request_repaint();
    }
}

fn dist_to_polyline(p: Pos2, pts: &[Pos2]) -> f32 {
    let mut best = f32::INFINITY;
    for w in pts.windows(2) {
        let ab = w[1] - w[0];
        let len2 = ab.length_sq();
        let t = if len2 <= 1e-9 { 0.0 } else { ((p - w[0]).dot(ab) / len2).clamp(0.0, 1.0) };
        best = best.min(p.distance(w[0] + ab * t));
    }
    best
}

/// Screen polylines for the parts of `segs` that are in front of the globe.
fn screen_runs(pr: &crate::camera::Projector, rect: egui::Rect, on_screen: egui::Rect, segs: &[Vec<V3>]) -> Vec<Vec<Pos2>> {
    let mut runs: Vec<Vec<Pos2>> = Vec::new();
    for seg in segs {
        let mut run: Vec<Pos2> = Vec::new();
        for &p in seg {
            let s = if pr.visible(p) { pr.project(p).map(|s| rect.min + egui::vec2(s[0] as f32, s[1] as f32)) } else { None };
            match s {
                Some(s) => {
                    if run.last().map_or(true, |l| l.distance(s) >= 0.8) {
                        run.push(s);
                    }
                }
                None => {
                    if run.len() >= 2 {
                        runs.push(std::mem::take(&mut run));
                    }
                    run.clear();
                }
            }
        }
        if run.len() >= 2 {
            runs.push(run);
        }
    }
    runs.retain(|r| {
        let (mut lo, mut hi) = (r[0], r[0]);
        for p in r {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        egui::Rect::from_min_max(lo, hi).intersects(on_screen)
    });
    runs
}

/// What `draw` found under the pointer.
#[derive(Default)]
pub struct Out {
    /// A caption for the track under the mouse.
    pub caption: Option<String>,
    /// The track under the mouse (index into the items).
    pub hover: Option<usize>,
    /// The track under the click, when there was one.
    pub click: Option<usize>,
}

/// Draws the shown tracks. `hover` and `click` are screen positions to test against the paths.
pub fn draw(painter: &egui::Painter, rect: egui::Rect, cam: &Camera, zf: f64, items: &[Item], view: View, selected: Option<usize>, hover: Option<Pos2>, click: Option<Pos2>) -> Out {
    let mut out = Out::default();
    if zf < 4.0 {
        return out;
    }
    let (w, h) = (rect.width() as f64, rect.height() as f64);
    let pr = cam.projector(w, h);
    let on_screen = rect.expand(60.0);
    let width = (2.6 + (zf - 6.0) * 0.4).clamp(2.6, 6.0) as f32;
    let mut best_hover: Option<(f32, usize)> = None;
    let mut best_click: Option<(f32, usize)> = None;
    let reach = (width * 0.5 + 6.0).max(8.0);
    for (idx, it) in items.iter().enumerate().filter(|(_, i)| i.shown) {
        let t = &it.track;
        if !circle_may_be_visible(cam, t.center, t.radius, w / h) {
            continue;
        }
        let col = Color32::from_rgb(t.color[0], t.color[1], t.color[2]);
        let is_sel = selected == Some(idx);
        let main_segs = t.main_segs(view);
        let runs = screen_runs(&pr, rect, on_screen, main_segs);
        let faint = if matches!(view, View::Both | View::Snapped) { screen_runs(&pr, rect, on_screen, &t.segs) } else { Vec::new() };
        let mut probe: Vec<&Vec<Pos2>> = runs.iter().collect();
        // the original, thin and see-through, under the tidied one
        for r in &faint {
            painter.add(egui::Shape::line(r.clone(), Stroke::new((width * 0.55).max(1.6), Color32::from_rgba_unmultiplied(t.color[0], t.color[1], t.color[2], 120))));
            probe.push(r);
        }
        let casing = if is_sel { width + 6.0 } else { width + 2.4 };
        let casing_col = if is_sel { Color32::from_rgba_unmultiplied(255, 255, 255, 255) } else { Color32::from_rgba_unmultiplied(255, 255, 255, 215) };
        for r in &runs {
            painter.add(egui::Shape::line(r.clone(), Stroke::new(casing, casing_col)));
        }
        for r in &runs {
            painter.add(egui::Shape::line(r.clone(), Stroke::new(width + if is_sel { 1.0 } else { 0.0 }, col)));
        }
        // start (green) and end (red), once the track is big enough on screen to matter
        if zf >= 9.0 {
            let ends = [(main_segs.first().and_then(|s| s.first()), Color32::from_rgb(46, 204, 113)), (main_segs.last().and_then(|s| s.last()), Color32::from_rgb(231, 76, 60))];
            for (p, c) in ends {
                if let Some(&p) = p {
                    if pr.visible(p) {
                        if let Some(s) = pr.project(p) {
                            let s = rect.min + egui::vec2(s[0] as f32, s[1] as f32);
                            if on_screen.contains(s) {
                                painter.circle(s, width * 0.9 + 1.5, c, Stroke::new(2.0_f32, Color32::WHITE));
                            }
                        }
                    }
                }
            }
        }
        for (at, best) in [(hover, &mut best_hover), (click, &mut best_click)] {
            if let Some(p) = at {
                let d = probe.iter().map(|r| dist_to_polyline(p, r)).fold(f32::INFINITY, f32::min);
                if d <= reach && best.as_ref().map_or(true, |b| d < b.0) {
                    *best = Some((d, idx));
                }
            }
        }
    }
    if let Some((_, i)) = best_hover {
        let t = &items[i].track;
        out.caption = Some(format!("{}  ·  {:.1} km  ·  click for details and replay", t.name, t.km));
        out.hover = Some(i);
    }
    out.click = best_click.map(|b| b.1);
    out
}

/// The walker during a replay: a pulsing dot on the path.
pub fn draw_marker(painter: &egui::Painter, rect: egui::Rect, cam: &Camera, pos: V3, color: [u8; 3], now: f64) {
    let pr = cam.projector(rect.width() as f64, rect.height() as f64);
    if !pr.visible(pos) {
        return;
    }
    if let Some(s) = pr.project(pos) {
        let s = rect.min + egui::vec2(s[0] as f32, s[1] as f32);
        if rect.expand(30.0).contains(s) {
            let pulse = (now * 3.0).sin() as f32 * 0.5 + 0.5;
            painter.circle_filled(s, 13.0 + 4.0 * pulse, Color32::from_rgba_unmultiplied(color[0], color[1], color[2], 70));
            painter.circle(s, 7.0, Color32::from_rgb(color[0], color[1], color[2]), Stroke::new(3.0_f32, Color32::WHITE));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KML: &str = "<kml><Placemark><name>Walk</name><LineString><coordinates>-122.30,37.90 -122.29,37.91 -122.28,37.92</coordinates></LineString></Placemark></kml>";

    #[test]
    fn a_newer_copy_of_a_file_replaces_the_old_one_in_place() {
        let dir = std::env::temp_dir().join(format!("rmaps-tracks-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut set = TrackSet::new(&egui::Context::default(), &dir, &dir);
        std::fs::write(set.dir().join("Phone_2026-11-28.kml"), KML).unwrap();
        set.reload_file("Phone_2026-11-28.kml");
        std::fs::write(set.dir().join("Other.kml"), KML).unwrap();
        set.reload_file("Other.kml");
        assert_eq!(set.items.len(), 2);
        set.items[0].shown = false;
        let color = set.items[0].track.color;
        std::fs::write(set.dir().join("Phone_2026-11-28.kml"), KML.replace("Walk", "Longer walk")).unwrap();
        set.reload_file("Phone_2026-11-28.kml");
        assert_eq!(set.items.len(), 2, "replaced, not added");
        assert_eq!(set.items[0].track.name, "Longer walk");
        assert!(!set.items[0].shown && set.items[0].track.color == color, "keeps what the user chose");
        let _ = std::fs::remove_dir_all(&dir);
    }

    const TIMED: &str = "<kml><Placemark><name>Run 10:00-10:01</name><gx:Track>\
        <when>2026-11-28T18:00:00Z</when><when>2026-11-28T18:00:10Z</when><when>2026-11-28T18:00:20Z</when>\
        <gx:coord>-122.3000 37.9000 0</gx:coord><gx:coord>-122.2990 37.9000 0</gx:coord><gx:coord>-122.2980 37.9000 0</gx:coord>\
        </gx:Track></Placemark></kml>";

    #[test]
    fn a_timed_file_replays_with_real_speeds_and_an_untimed_one_with_an_estimate() {
        use crate::replay::Timing;
        let dir = std::env::temp_dir().join(format!("rmaps-tracks-time-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut set = TrackSet::new(&egui::Context::default(), &dir, &dir);
        std::fs::write(set.dir().join("Timed.kml"), TIMED).unwrap();
        set.reload_file("Timed.kml");
        let old = "<kml><Placemark><name>Walk 10:00-10:01</name><LineString><coordinates>-122.3000,37.9000 -122.2990,37.9000 -122.2980,37.9000</coordinates></LineString></Placemark></kml>";
        std::fs::write(set.dir().join("Old.kml"), old).unwrap();
        set.reload_file("Old.kml");
        assert_eq!(set.items.len(), 2);
        let t = &set.items[0].track;
        assert_eq!(t.orig.timing, Timing::Exact);
        assert!((t.orig.total_secs() - 20.0).abs() < 1e-9);
        assert!(t.orig.max_ms() > 5.0 && t.orig.max_ms() < 12.0, "{}", t.orig.max_ms());
        assert_eq!(t.cleaned.timing, Timing::Exact);
        // no per-point times, but the name says 10:00-10:01: one minute, spread evenly
        let o = &set.items[1].track;
        assert_eq!(o.orig.timing, Timing::Estimated);
        assert!((o.orig.total_secs() - 60.0).abs() < 1e-9);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
