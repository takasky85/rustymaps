//! Tile loading: disk cache, polite parallel downloads, mesh building on worker
//! threads, GPU upload and eviction, and choosing which tiles to draw.
//!
//! The UI thread never waits on the network or on mesh building. Each frame it
//! (1) takes a few finished meshes and uploads them, (2) works out which tiles
//! cover the screen, (3) hands the workers an updated, prioritised to-do list.
//! Tiles that are not ready yet are covered by their nearest loaded ancestor.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

use eframe::{egui, egui_wgpu};

use crate::camera::{dot, lonlat_to_merc, merc_to_vec, Camera, FOV_Y};
use crate::gpu::MapGpu;
use crate::mesh::{self, Label, TileMesh};
use crate::tile_id::TileId;

pub const TILE_URL: &str = "https://tiles.openfreemap.org/planet/latest/{z}/{x}/{y}.pbf";
pub const USER_AGENT: &str = "rustymaps/0.1 (+https://github.com/takasky85/rustymaps)";
pub const MAX_TILE_ZOOM: u8 = 14;
/// Parallel downloads, kept small on purpose.
const WORKERS: usize = 4;
/// Tiles kept decoded in memory / on the GPU.
const MAX_ENTRIES: usize = 700;
/// Meshes uploaded to the GPU per frame, to avoid hitches.
const MAX_UPLOADS_PER_FRAME: usize = 12;
/// Upper bound on tiles considered per frame.
const MAX_VISIBLE: usize = 100;
/// Disk cache size limit.
const CACHE_CAP_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Cached tiles older than this are fetched again.
const CACHE_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);

// ---------------------------------------------------------------------------
// Disk cache
// ---------------------------------------------------------------------------

pub struct DiskCache {
    root: PathBuf,
    puts: AtomicUsize,
    trimming: AtomicBool,
}

impl DiskCache {
    pub fn new() -> Self {
        DiskCache { root: default_cache_dir(), puts: AtomicUsize::new(0), trimming: AtomicBool::new(false) }
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    fn path(&self, id: TileId) -> PathBuf {
        self.root.join(id.z.to_string()).join(id.x.to_string()).join(format!("{}.mvt", id.y))
    }

    /// `Some(bytes)` on a fresh hit; an empty vec means "this tile is known to be empty".
    pub fn get(&self, id: TileId) -> Option<Vec<u8>> {
        let p = self.path(id);
        let meta = std::fs::metadata(&p).ok()?;
        let age = SystemTime::now().duration_since(meta.modified().ok()?).unwrap_or_default();
        if age > CACHE_MAX_AGE {
            return None;
        }
        std::fs::read(&p).ok()
    }

    pub fn put(&self, id: TileId, bytes: &[u8]) {
        let p = self.path(id);
        if let Some(dir) = p.parent() {
            if std::fs::create_dir_all(dir).is_err() {
                return;
            }
        }
        let tmp = p.with_extension(format!("tmp{:?}", std::thread::current().id()).replace(['(', ')'], ""));
        if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, &p).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        if self.puts.fetch_add(1, Ordering::Relaxed) % 400 == 399 {
            self.trim();
        }
    }

    pub fn remove(&self, id: TileId) {
        let _ = std::fs::remove_file(self.path(id));
    }

    pub fn clear(&self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }

    /// Deletes the oldest files until the cache is under ~90% of its cap.
    pub fn trim(&self) {
        if self.trimming.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let Ok(md) = e.metadata() else { continue };
                if md.is_dir() {
                    stack.push(e.path());
                } else {
                    files.push((md.modified().unwrap_or(SystemTime::UNIX_EPOCH), md.len(), e.path()));
                }
            }
        }
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        if total > CACHE_CAP_BYTES {
            files.sort_by_key(|f| f.0);
            let target = CACHE_CAP_BYTES / 10 * 9;
            for (_, len, path) in files {
                if total <= target {
                    break;
                }
                if std::fs::remove_file(&path).is_ok() {
                    total = total.saturating_sub(len);
                }
            }
        }
        self.trimming.store(false, Ordering::Release);
    }
}

fn default_cache_dir() -> PathBuf {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if cfg!(windows) {
        env("LOCALAPPDATA").or_else(|| env("TEMP"))
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library").join("Caches"))
    } else {
        env("XDG_CACHE_HOME").or_else(|| env("HOME").map(|h| h.join(".cache")))
    };
    base.unwrap_or_else(std::env::temp_dir).join("rustymaps").join("tiles")
}

// ---------------------------------------------------------------------------
// Workers
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Inner {
    /// Highest priority first.
    queue: VecDeque<TileId>,
    active: HashSet<TileId>,
    results: VecDeque<(TileId, Result<Option<TileMesh>, String>)>,
}

struct Shared {
    inner: Mutex<Inner>,
    cv: Condvar,
    quit: AtomicBool,
}

fn fetch(agent: &ureq::Agent, id: TileId) -> Result<Vec<u8>, String> {
    let url = TILE_URL
        .replace("{z}", &id.z.to_string())
        .replace("{x}", &id.x.to_string())
        .replace("{y}", &id.y.to_string());
    let mut resp = agent.get(&url).call().map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    if status == 404 || status == 204 {
        return Ok(Vec::new());
    }
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {status}"));
    }
    let body = resp.body_mut().read_to_vec().map_err(|e| e.to_string())?;
    if body.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(&body[..]).read_to_end(&mut out).map_err(|e| e.to_string())?;
        return Ok(out);
    }
    Ok(body)
}

fn load_tile(id: TileId, cache: &DiskCache, agent: &ureq::Agent) -> Result<Option<TileMesh>, String> {
    let bytes = match cache.get(id) {
        Some(b) => b,
        None => {
            let b = fetch(agent, id)?;
            cache.put(id, &b);
            b
        }
    };
    if bytes.is_empty() {
        return Ok(None);
    }
    match mesh::build_mesh(id, &bytes) {
        Ok(m) => Ok(Some(m)),
        Err(e) => {
            cache.remove(id);
            Err(e)
        }
    }
}

fn worker(shared: Arc<Shared>, cache: Arc<DiskCache>, ctx: egui::Context) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .user_agent(USER_AGENT)
        .http_status_as_error(false)
        .build()
        .into();
    loop {
        let id = {
            let mut g = shared.inner.lock().unwrap();
            loop {
                if shared.quit.load(Ordering::Acquire) {
                    return;
                }
                if let Some(id) = g.queue.pop_front() {
                    g.active.insert(id);
                    break id;
                }
                g = shared.cv.wait(g).unwrap();
            }
        };
        let res = load_tile(id, &cache, &agent);
        {
            let mut g = shared.inner.lock().unwrap();
            g.active.remove(&id);
            g.results.push_back((id, res));
        }
        ctx.request_repaint();
    }
}

// ---------------------------------------------------------------------------
// Manager (UI thread)
// ---------------------------------------------------------------------------

struct Entry {
    on_gpu: bool,
    labels: Arc<Vec<Label>>,
    last_used: u64,
}

/// What the app needs to draw one frame.
pub struct FrameTiles {
    /// Tiles to draw, coarse to fine.
    pub draw: Vec<TileId>,
    /// Labels of the drawn tiles (duplicates across zoom levels are fine; collision removes them).
    pub labels: Vec<Arc<Vec<Label>>>,
    pub pending: usize,
    pub loaded: usize,
    pub failed: usize,
}

pub struct TileManager {
    ctx: egui::Context,
    rs: egui_wgpu::RenderState,
    shared: Arc<Shared>,
    cache: Arc<DiskCache>,
    loaded: HashMap<TileId, Entry>,
    failed: HashMap<TileId, (Instant, u32)>,
    frame: u64,
}

impl TileManager {
    pub fn new(ctx: &egui::Context, rs: egui_wgpu::RenderState) -> Self {
        rs.renderer.write().callback_resources.insert(MapGpu::new(&rs.device, rs.target_format));

        let cache = Arc::new(DiskCache::new());
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner::default()),
            cv: Condvar::new(),
            quit: AtomicBool::new(false),
        });
        for i in 0..WORKERS {
            let (s, c, x) = (shared.clone(), cache.clone(), ctx.clone());
            let _ = std::thread::Builder::new().name(format!("tile-worker-{i}")).spawn(move || worker(s, c, x));
        }
        {
            // Keep the cache within its cap, without delaying startup.
            let c = cache.clone();
            let _ = std::thread::Builder::new().name("cache-trim".into()).spawn(move || c.trim());
        }
        TileManager { ctx: ctx.clone(), rs, shared, cache, loaded: HashMap::new(), failed: HashMap::new(), frame: 0 }
    }

    pub fn cache_dir(&self) -> &PathBuf {
        self.cache.root()
    }

    pub fn clear_cache(&self) {
        self.cache.clear();
    }

    /// `w`, `h` are the map viewport in logical points.
    pub fn update(&mut self, cam: &Camera, w: f64, h: f64) -> FrameTiles {
        self.frame += 1;
        let now = Instant::now();

        // 1. Take a few finished tiles and upload them.
        let finished: Vec<_> = {
            let mut g = self.shared.inner.lock().unwrap();
            let n = g.results.len().min(MAX_UPLOADS_PER_FRAME);
            g.results.drain(..n).collect()
        };
        if !finished.is_empty() {
            let mut renderer = self.rs.renderer.write();
            let gpu = renderer.callback_resources.get_mut::<MapGpu>();
            if let Some(gpu) = gpu {
                for (id, res) in finished {
                    match res {
                        Ok(mesh) => {
                            self.failed.remove(&id);
                            let entry = match mesh {
                                Some(mut m) => {
                                    let on_gpu = !m.fills.is_empty() || !m.lines.is_empty();
                                    if on_gpu {
                                        gpu.add_tile(&self.rs.device, id, &m);
                                    }
                                    Entry { on_gpu, labels: Arc::new(std::mem::take(&mut m.labels)), last_used: self.frame }
                                }
                                None => Entry { on_gpu: false, labels: Arc::new(Vec::new()), last_used: self.frame },
                            };
                            self.loaded.insert(id, entry);
                        }
                        Err(e) => {
                            let n = self.failed.get(&id).map_or(1, |f| f.1 + 1);
                            let delay = (8u64 << (n - 1).min(3)).min(60);
                            eprintln!("tile {}/{}/{}: {e} (retry in {delay}s)", id.z, id.x, id.y);
                            self.failed.insert(id, (now + Duration::from_secs(delay), n));
                        }
                    }
                }
            }
        }
        // 2. Which tiles cover the screen?
        let zf = cam.zoom_f(h);
        let z = (zf.round().max(0.0) as u8).min(MAX_TILE_ZOOM);
        let targets = visible_tiles(cam, w, h, z, zf);

        let mut wanted: Vec<TileId> = targets.clone();
        let mut seen: HashSet<TileId> = wanted.iter().copied().collect();
        for t in &targets {
            if let Some(p) = t.parent() {
                if seen.insert(p) {
                    wanted.push(p);
                }
            }
        }

        // 3. Hand the workers an up-to-date to-do list (replacing the old one cancels stale work).
        let todo: Vec<TileId> = wanted
            .iter()
            .copied()
            .filter(|t| !self.loaded.contains_key(t))
            .filter(|t| self.failed.get(t).map_or(true, |f| f.0 <= now))
            .collect();
        let pending = {
            let mut g = self.shared.inner.lock().unwrap();
            let in_results: HashSet<TileId> = g.results.iter().map(|r| r.0).collect();
            let queue: VecDeque<TileId> =
                todo.into_iter().filter(|t| !g.active.contains(t) && !in_results.contains(t)).collect();
            g.queue = queue;
            g.queue.len() + g.active.len() + g.results.len()
        };
        self.shared.cv.notify_all();
        if pending > 0 || !self.failed.is_empty() {
            // Workers wake the UI when a tile finishes; this covers queued
            // results beyond the per-frame upload budget and failed-tile retries.
            self.ctx.request_repaint_after(Duration::from_millis(if pending > 0 { 30 } else { 1000 }));
        }

        // 4. For each target tile draw the nearest loaded tile (itself or an ancestor).
        let mut draw_set: HashSet<TileId> = HashSet::new();
        for t in &targets {
            let mut cur = Some(*t);
            while let Some(c) = cur {
                if let Some(e) = self.loaded.get_mut(&c) {
                    e.last_used = self.frame;
                    draw_set.insert(c);
                    break;
                }
                cur = c.parent();
            }
        }
        for p in &wanted {
            if let Some(e) = self.loaded.get_mut(p) {
                e.last_used = self.frame;
            }
        }
        let mut order: Vec<TileId> = draw_set.into_iter().collect();
        order.sort();
        order.sort_by_key(|t| t.z);

        let mut draw = Vec::new();
        let mut labels = Vec::new();
        for id in &order {
            let e = &self.loaded[id];
            if e.on_gpu {
                draw.push(*id);
            }
            if !e.labels.is_empty() {
                labels.push(e.labels.clone());
            }
        }

        // 5. Evict least recently used tiles when over budget.
        if self.loaded.len() > MAX_ENTRIES {
            let mut victims: Vec<(u64, TileId)> =
                self.loaded.iter().filter(|(_, e)| e.last_used < self.frame).map(|(k, e)| (e.last_used, *k)).collect();
            victims.sort();
            let excess = self.loaded.len() - (MAX_ENTRIES - MAX_ENTRIES / 7);
            let mut renderer = self.rs.renderer.write();
            let mut gpu = renderer.callback_resources.get_mut::<MapGpu>();
            for (_, id) in victims.into_iter().take(excess) {
                if let Some(g) = gpu.as_mut() {
                    g.remove_tile(&id);
                }
                self.loaded.remove(&id);
            }
        }

        FrameTiles { draw, labels, pending, loaded: self.loaded.len(), failed: self.failed.len() }
    }
}

impl Drop for TileManager {
    fn drop(&mut self) {
        self.shared.quit.store(true, Ordering::Release);
        self.shared.cv.notify_all();
    }
}

// ---------------------------------------------------------------------------
// Visibility
// ---------------------------------------------------------------------------

/// Tiles at zoom `z` that may be visible, nearest to the view centre first.
pub fn visible_tiles(cam: &Camera, w: f64, h: f64, z: u8, zf: f64) -> Vec<TileId> {
    let n = 1u32 << z;
    let (mx, my) = lonlat_to_merc(cam.lon, cam.lat);
    let cx = (mx * n as f64).floor() as i64;
    let cy = ((my * n as f64).floor() as i64).clamp(0, n as i64 - 1);

    // Tile size on screen at the view centre, in points.
    let tile_px = 512.0 * 2f64.powf(zf - z as f64);
    let half_diag = 0.5 * (w * w + h * h).sqrt();
    // Away from the centre the globe is foreshortened, so more tiles fit.
    let k = 1.0 + cam.alt.min(1.0) * 1.5;
    let r = ((half_diag / tile_px * k).ceil() as i64 + 1).clamp(1, 8);

    let dir = cam.dir();
    let horizon = (1.0 / cam.dist()).clamp(-1.0, 1.0).acos();
    let px_per_rad = h / (2.0 * (FOV_Y * 0.5).tan() * cam.alt.max(1e-9));
    let size = 1.0 / n as f64;
    let tile_rad = std::f64::consts::TAU * size * 0.75;
    let margin = (tile_rad * px_per_rad * 1.5 + 64.0).min(8.0 * w.max(h));

    let mut seen = HashSet::new();
    let mut out: Vec<(f64, TileId)> = Vec::new();
    for dy in -r..=r {
        let y = cy + dy;
        if y < 0 || y >= n as i64 {
            continue;
        }
        for dx in -r..=r {
            let x = (cx + dx).rem_euclid(n as i64) as u32;
            let id = TileId { z, x, y: y as u32 };
            if !seen.insert(id) {
                continue;
            }
            let (tmx, tmy) = id.center_merc();
            let c = merc_to_vec(tmx, tmy);
            let ang = dot(c, dir).clamp(-1.0, 1.0).acos();
            if ang > horizon + tile_rad {
                continue;
            }
            if ang > tile_rad {
                match cam.project(c, w, h) {
                    Some(p) => {
                        if p[0] < -margin || p[0] > w + margin || p[1] < -margin || p[1] > h + margin {
                            continue;
                        }
                    }
                    None => continue,
                }
            }
            out.push((ang, id));
        }
    }
    out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(MAX_VISIBLE);
    out.into_iter().map(|t| t.1).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_view_needs_few_tiles() {
        let cam = Camera::from_degrees(-122.4, 37.0, 2.2);
        let h = 800.0;
        let zf = cam.zoom_f(h);
        let z = zf.round() as u8;
        let t = visible_tiles(&cam, 1280.0, h, z, zf);
        assert!(!t.is_empty() && t.len() <= MAX_VISIBLE, "{} tiles at z{}", t.len(), z);
    }

    #[test]
    fn city_view_includes_home_tile_first() {
        let h = 800.0;
        let alt = Camera::alt_for_zoom(12.5, 37.77f64.to_radians(), h);
        let cam = Camera::from_degrees(-122.42, 37.77, alt);
        let zf = cam.zoom_f(h);
        let z = zf.round() as u8;
        let t = visible_tiles(&cam, 1280.0, h, z, zf);
        let (mx, my) = lonlat_to_merc(cam.lon, cam.lat);
        let n = (1u32 << z) as f64;
        let home = TileId { z, x: (mx * n) as u32, y: (my * n) as u32 };
        assert!(t.contains(&home));
        assert!(t.len() < 40, "{}", t.len());
    }

    #[test]
    fn antimeridian_wraps_without_duplicates() {
        let cam = Camera::from_degrees(179.9, 0.0, 0.3);
        let h = 800.0;
        let zf = cam.zoom_f(h);
        let t = visible_tiles(&cam, 1280.0, h, zf.round() as u8, zf);
        let set: HashSet<_> = t.iter().collect();
        assert_eq!(set.len(), t.len());
    }
}
