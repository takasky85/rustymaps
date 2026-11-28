//! GPS tracks (KML files from the phone tracker) kept in `data/tracks/` and drawn over the map.
//! Files are read on a worker thread; the folder is the whole database, so copying a `.kml` in
//! (or, later, the phone sending one) is all it takes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, Pos2, Stroke};

use crate::camera::{vec_to_lonlat, Camera, V3};
use crate::kml;
use crate::transit::circle_may_be_visible;

pub struct Track {
    pub name: String,
    /// File name inside the tracks folder.
    pub file: String,
    pub segs: Vec<Vec<V3>>,
    pub center: V3,
    /// Angular radius (radians) around `center` that holds every point.
    pub radius: f64,
    pub km: f64,
    pub color: [u8; 3],
}

impl Track {
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
    Track { name: p.name, file: file.to_string(), segs: p.segs, center, radius, km, color: [0; 3] }
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

impl TrackSet {
    pub fn new(ctx: &egui::Context, data_dir: &Path) -> TrackSet {
        let dir = data_dir.join("tracks");
        let _ = std::fs::create_dir_all(&dir);
        let set = TrackSet { ctx: ctx.clone(), dir: dir.clone(), items: Vec::new(), incoming: Arc::new(Mutex::new(Vec::new())), message: String::new() };
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

/// Draws the shown tracks. Returns a short caption for the track under `hover`, if any.
pub fn draw(painter: &egui::Painter, rect: egui::Rect, cam: &Camera, zf: f64, items: &[Item], hover: Option<Pos2>) -> Option<String> {
    if zf < 4.0 {
        return None;
    }
    let (w, h) = (rect.width() as f64, rect.height() as f64);
    let pr = cam.projector(w, h);
    let on_screen = rect.expand(60.0);
    let width = (2.6 + (zf - 6.0) * 0.4).clamp(2.6, 6.0) as f32;
    let mut caption: Option<(f32, String)> = None;
    for it in items.iter().filter(|i| i.shown) {
        let t = &it.track;
        if !circle_may_be_visible(cam, t.center, t.radius, w / h) {
            continue;
        }
        let col = Color32::from_rgb(t.color[0], t.color[1], t.color[2]);
        let mut runs: Vec<Vec<Pos2>> = Vec::new();
        for seg in &t.segs {
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
        for r in &runs {
            painter.add(egui::Shape::line(r.clone(), Stroke::new(width + 2.4, Color32::from_rgba_unmultiplied(255, 255, 255, 215))));
        }
        for r in &runs {
            painter.add(egui::Shape::line(r.clone(), Stroke::new(width, col)));
        }
        // start (green) and end (red), once the track is big enough on screen to matter
        if zf >= 9.0 {
            let ends = [(t.segs.first().and_then(|s| s.first()), Color32::from_rgb(46, 204, 113)), (t.segs.last().and_then(|s| s.last()), Color32::from_rgb(231, 76, 60))];
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
        if let Some(hp) = hover {
            let d = runs.iter().map(|r| dist_to_polyline(hp, r)).fold(f32::INFINITY, f32::min);
            if d <= (width * 0.5 + 6.0).max(8.0) && caption.as_ref().map_or(true, |c| d < c.0) {
                caption = Some((d, format!("{}  ·  {:.1} km", t.name, t.km)));
            }
        }
    }
    caption.map(|c| c.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KML: &str = "<kml><Placemark><name>Walk</name><LineString><coordinates>-122.30,37.90 -122.29,37.91 -122.28,37.92</coordinates></LineString></Placemark></kml>";

    #[test]
    fn a_newer_copy_of_a_file_replaces_the_old_one_in_place() {
        let dir = std::env::temp_dir().join(format!("rmaps-tracks-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut set = TrackSet::new(&egui::Context::default(), &dir);
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
}
