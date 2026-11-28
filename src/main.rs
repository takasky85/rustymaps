//! RustyMaps: a small globe-to-street-level vector map viewer.
//!
//! Rust + egui + wgpu. Tiles come from OpenFreeMap (vector, OpenMapTiles schema),
//! are drawn on the GPU by a custom paint callback, and labels are drawn by egui.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod camera;
mod clean;
mod data;
mod detail;
mod flight;
mod fly;
mod fonts;
mod globe;
mod gpu;
mod json;
mod kml;
mod layers;
mod mesh;
mod mvt;
mod phonesync;
mod plane;
mod replay;
mod search;
mod snap;
mod snap_tiles;
mod settings;
mod sha256;
mod style;
mod tile_id;
mod tiles;
mod trackinfo;
mod tracks;
mod transit;
mod transit_data;
mod ui;

use std::sync::Arc;

use eframe::{egui, egui_wgpu, wgpu};

use camera::{dot, merc_to_vec, Camera, Projector, MAX_ALT, MIN_ALT, V3};
use fly::{Fly, FLY_SPEEDS};
use flight::{AircraftDef, Route, Sim, AIRCRAFT};
use gpu::{Globals, MapCallback};
use plane::{PlaneCallback, PlaneDraw};
use style::Palette;
use tiles::TileManager;
use transit::{StationLabel, TransitManager};
use transit_data::{SystemData, SYSTEMS};

const CITY_ZOOM: f64 = 12.5;

/// Picks the most capable GPU: discrete before integrated before software,
/// and on a tie DX12/Metal before Vulkan before GL.
fn select_adapter(adapters: &[wgpu::Adapter], surface: Option<&wgpu::Surface<'_>>) -> Result<wgpu::Adapter, String> {
    let score = |a: &wgpu::Adapter| {
        let info = a.get_info();
        let kind = match info.device_type {
            wgpu::DeviceType::DiscreteGpu => 4,
            wgpu::DeviceType::IntegratedGpu => 3,
            wgpu::DeviceType::VirtualGpu => 2,
            _ => 1,
        };
        let backend = match info.backend {
            wgpu::Backend::Dx12 | wgpu::Backend::Metal => 3,
            wgpu::Backend::Vulkan => 2,
            _ => 1,
        };
        (kind, backend)
    };
    let best = adapters
        .iter()
        .filter(|a| surface.map_or(true, |s| a.is_surface_supported(s)))
        .max_by_key(|a| score(a))
        .cloned()
        .ok_or_else(|| "no suitable GPU adapter found".to_string())?;
    let info = best.get_info();
    eprintln!("GPU: {} ({:?}, {:?})", info.name, info.device_type, info.backend);
    let _ = ACTIVE_GPU.set(format!("{} via {:?}", info.name, info.backend));
    Ok(best)
}

/// The GPU and graphics API in use (for the View tab).
static ACTIVE_GPU: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Graphics APIs the user can choose between (applied at start-up).
const BACKENDS: [(&str, wgpu::Backends); 4] = [
    ("Automatic", wgpu::Backends::PRIMARY),
    ("Vulkan", wgpu::Backends::VULKAN),
    ("DirectX 12", wgpu::Backends::DX12),
    ("OpenGL", wgpu::Backends::GL),
];

fn native_options(backends: wgpu::Backends) -> eframe::NativeOptions {
    // Start from egui's own defaults (instance flags, backend options) and change only what we need.
    let mut setup = egui_wgpu::WgpuSetupCreateNew::default();
    if backends != wgpu::Backends::PRIMARY {
        setup.instance_descriptor.backends = backends;
    }
    setup.power_preference = wgpu::PowerPreference::HighPerformance;
    setup.native_adapter_selector = Some(Arc::new(select_adapter));
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]).with_title("RustyMaps"),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: egui_wgpu::WgpuConfiguration {
            wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(setup),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// The release build has no console window, so a crash would be silent. Write what happened to
/// `crash.txt` in the user data folder (next to settings.txt) so it can be read afterwards.
fn install_crash_log() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let text = format!("RustyMaps crashed:\n{info}\n\n{}\n", std::backtrace::Backtrace::force_capture());
        let _ = std::fs::create_dir_all(crate::data::user_data_dir());
        let _ = std::fs::write(crate::data::user_data_dir().join("crash.txt"), text);
        default_hook(info);
    }));
}

/// Record a startup failure that is returned as an error (not a panic) in the same file.
fn log_startup_error(what: &str) {
    let _ = std::fs::create_dir_all(crate::data::user_data_dir());
    let _ = std::fs::write(crate::data::user_data_dir().join("crash.txt"), format!("RustyMaps could not start:\n{what}\n"));
}

fn main() -> eframe::Result {
    install_crash_log();
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--fetch-transit") {
        let dir = args.get(i + 1).map(String::as_str).unwrap_or("transit");
        let mut only: Vec<String> = Vec::new();
        let mut max = 0usize;
        let mut rest = args.iter().skip(i + 2);
        while let Some(a) = rest.next() {
            if a == "--max" {
                max = rest.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            } else {
                only.push(a.clone());
            }
        }
        transit::fetch_to_dir(std::path::Path::new(dir), &only, max);
        return Ok(());
    }
    // The graphics API chosen in the View tab. If it cannot start on this computer, fall back to
    // automatic (and forget the choice) instead of leaving the app unable to open.
    let chosen = settings::Store::load().get::<usize>("backend").unwrap_or(0).min(BACKENDS.len() - 1);
    let run = |backends| eframe::run_native("RustyMaps", native_options(backends), Box::new(|cc| Ok(Box::new(App::new(cc)))));
    let result = match run(BACKENDS[chosen].1) {
        Err(e) if chosen != 0 => {
            eprintln!("{} did not start ({e}); using automatic", BACKENDS[chosen].0);
            let mut st = settings::Store::load();
            st.set("backend", 0);
            let _ = std::fs::write(settings::path(), st.text());
            run(BACKENDS[0].1)
        }
        other => other,
    };
    if let Err(e) = &result {
        log_startup_error(&e.to_string());
    }
    result
}

/// Playback speeds for the simulated flights (multiples of real time).
const SPEEDS: [(&str, f64); 4] = [("real time", 1.0), ("60x", 60.0), ("600x", 600.0), ("3600x", 3600.0)];

/// Speeds for the simulated trains (multiples of real time).
const TRAIN_SPEEDS: [(&str, f64); 3] = [("real time", 1.0), ("5x", 5.0), ("20x", 20.0)];

/// Label size choices: (name, multiplier).
const LABEL_SIZES: [(&str, f32); 4] = [("Small", 0.85), ("Medium", 1.0), ("Large", 1.3), ("X-large", 1.65)];

struct Stats {
    drawn: usize,
    loaded: usize,
    pending: usize,
    failed: usize,
}

/// One simulated aircraft.
struct Plane {
    def: &'static AircraftDef,
    route: Route,
    sim: Sim,
    show: bool,
    /// Radius of the loaded model (metres); `None` when the model file was not found.
    radius_m: Option<f64>,
}

/// A platform sign on screen: for a station, either just hovered or pinned by a click.
struct Sign {
    sys: Arc<SystemData>,
    st: usize,
    /// Which of the station's lines the sign shows (clicking the pinned station again moves on).
    line: usize,
    pinned: bool,
}

struct App {
    cam: Camera,
    tiles: TileManager,
    transit: TransitManager,
    palette: Palette,
    fly: Option<Fly>,
    srgb: bool,
    gpu_name: String,
    fps: f64,
    /// Frames, map redraws and plane redraws per second, measured over the last second.
    rates: [u32; 3],
    rate_since: (f64, u32, u64, u64),
    stats: Stats,
    view_h: f64,
    data_dir: std::path::PathBuf,
    model_errors: Vec<String>,
    // labels
    show_labels: bool,
    label_size: usize,
    label_bold: bool,
    fonts: fonts::Fonts,
    show_compass: bool,
    // flights
    planes: Vec<Plane>,
    speed: usize,
    plane_size: f64,
    exaggeration: f64,
    follow: Option<usize>,
    route_color: [u8; 3],
    route_alpha: f32,
    route_width: f32,
    route_at_altitude: bool,
    transit_focus: bool,
    map_dim: f32,
    flat_stations: bool,
    show_trains: bool,
    train_speed: usize,
    /// Index into FLY_SPEEDS.
    fly_speed: usize,
    /// Index into BACKENDS; used the next time the app starts.
    backend: usize,
    /// Frame rate of animations (planes, trains) when nothing else is happening.
    anim_fps: u32,
    /// Time of the previous frame, for advancing the flights by real elapsed time.
    last_anim: f64,
    labels_shown: std::collections::HashSet<u64>,
    land: globe::Land,
    /// The mini globe's picture, redrawn only when the view turns.
    globe_tex: Option<egui::TextureHandle>,
    globe_key: [f64; 4],
    /// Local station names with English underneath (where both exist).
    bilingual: bool,
    /// The platform sign being shown for a station.
    sign: Option<Sign>,
    /// Ids of favourite transit systems (Places tab).
    favorites: Vec<String>,
    places_filter: String,
    /// The "transit here" list in the upper right is folded away.
    systems_folded: bool,
    /// Systems whose lines are listed (opened like folders) in that list.
    systems_open: std::collections::HashSet<usize>,
    /// A row of that list is being dragged.
    dnd: Option<layers::Dnd>,
    /// Layers: the whole transit map, and the GPS paths, can be switched off.
    show_transit: bool,
    show_tracks: bool,
    layers_open: bool,
    tracks: tracks::TrackSet,
    /// Which parts of the map are drawn (switched in the Layers panel).
    detail: detail::MapDetail,
    /// Which version of the GPS paths is drawn (the original, the tidied copy, or both).
    track_view: tracks::View,
    /// The clicked GPS path and its replay.
    player: Option<tracks::Player>,
    /// Receive tracks from the phone tracker over WiFi (off until you turn it on).
    phone_sync_on: bool,
    phone_sync: Option<phonesync::PhoneSync>,
    /// Camera as of the previous frame, to tell when the view is moving.
    prev_cam: [f64; 7],
    pacer: Pacer,
    /// Frames in the last second that were caused by mouse/keyboard input.
    input_frames: u32,
    input_rate: u32,
    /// Index into `ui::UI_SIZES`.
    ui_size: usize,
    /// 0 = smooth (supersampled) plane edges, 1 = fast.
    plane_quality: usize,
    /// Which tab of the control bar is open.
    tab: Option<ui::Tab>,
    search_text: String,
    search: search::Search,
    local_hits: Vec<search::Hit>,
    search_open: bool,
    /// The highlighted subway line: its system and the line's index in it.
    selected_line: Option<(Arc<SystemData>, usize)>,
    saved_settings: String,
    saved_at: f64,
}

/// How route lines are drawn (adjustable in the Flights section).
struct RouteStyle {
    color: [u8; 3],
    alpha: f32,
    width: f32,
    /// Draw the line at the plane's altitude instead of on the ground.
    at_altitude: bool,
    exaggeration: f64,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let rs = cc.wgpu_render_state.clone().expect("the wgpu renderer is required");
        let info = rs.adapter.get_info();
        let gpu_name = format!("{} ({:?})", info.name, info.backend);
        let srgb = rs.target_format.is_srgb();
        let tiles = TileManager::new(&cc.egui_ctx, rs.clone());
        // Label font: one of the fonts already installed on this computer (switchable in the UI).
        let fonts = fonts::Fonts::detect();
        fonts.apply(&cc.egui_ctx);

        // Aircraft models and transit maps come from the offline data folder.
        let data_dir = data::data_dir();
        let mut model_errors = Vec::new();
        let planes = AIRCRAFT
            .iter()
            .enumerate()
            .map(|(i, def)| {
                let radius_m = match data::load_model(&data_dir, def.model) {
                    Ok(m) => {
                        plane::upload(&rs, i, &m);
                        Some(m.radius_m)
                    }
                    Err(e) => {
                        eprintln!("model: {e}");
                        model_errors.push(e);
                        None
                    }
                };
                Plane { def, route: Route::of(def), sim: Sim::new(0.35), show: false, radius_m }
            })
            .collect();

        // Every city whose transit map is saved in the data folder is on from the start (the files
        // are small and load on worker threads); it shows as a metro symbol until you zoom in.
        let mut transit = TransitManager::new(&cc.egui_ctx, data_dir.clone());
        for i in 0..SYSTEMS.len() {
            if transit.offline(i) {
                transit.set_enabled(i, true);
            }
        }

        let tracks = tracks::TrackSet::new(&cc.egui_ctx, &data::user_data_dir(), &data_dir);
        let mut app = App {
            cam: Camera::from_degrees(-115.0, 32.0, 2.2),
            transit,
            tracks,
            systems_open: Default::default(),
            dnd: None,
            show_transit: true,
            show_tracks: true,
            detail: detail::MapDetail::default(),
            track_view: tracks::View::Both,
            player: None,
            layers_open: false,
            phone_sync_on: false,
            phone_sync: None,
            tiles,
            palette: Palette::STANDARD,
            fly: None,
            srgb,
            gpu_name,
            fps: 60.0,
            rates: [0; 3],
            rate_since: (0.0, 0, 0, 0),
            stats: Stats { drawn: 0, loaded: 0, pending: 0, failed: 0 },
            view_h: 800.0,
            data_dir,
            model_errors,
            show_labels: true,
            label_size: 1,
            label_bold: false,
            fonts,
            show_compass: true,
            planes,
            speed: 2,
            plane_size: 300.0,
            exaggeration: 12.0,
            follow: None,
            route_color: [40, 120, 255],
            route_alpha: 0.55,
            route_width: 3.0,
            route_at_altitude: true,
            transit_focus: true,
            map_dim: 0.5,
            flat_stations: true,
            show_trains: true,
            train_speed: 0,
            backend: 0,
            fly_speed: 1,
            anim_fps: 60,
            last_anim: 0.0,
            labels_shown: Default::default(),
            land: globe::Land::load(),
            globe_tex: None,
            globe_key: [f64::NAN; 4],
            bilingual: true,
            sign: None,
            favorites: Vec::new(),
            places_filter: String::new(),
            systems_folded: false,
            prev_cam: [0.0; 7],
            pacer: Pacer::new(&cc.egui_ctx),
            input_frames: 0,
            input_rate: 0,
            ui_size: 1,
            plane_quality: 0,
            tab: None,
            search_text: String::new(),
            search: search::Search::default(),
            local_hits: Vec::new(),
            search_open: false,
            selected_line: None,
            saved_settings: String::new(),
            saved_at: 0.0,
        };
        // Settings from the last run (kept in the user data folder, so they survive new builds).
        app.apply_settings(&settings::Store::load(), &cc.egui_ctx);
        if app.phone_sync_on {
            app.set_phone_sync(&cc.egui_ctx, true);
        }
        app.saved_settings = app.settings_text();
        app
    }

    /// Smooth move to `to` (see `Fly`). `min_duration` is a rough floor for short hops; the Fly speed
    /// setting scales everything, and "Jump" goes straight there.
    fn fly_to(&mut self, to: Camera, now: f64, min_duration: f64) {
        self.follow = None;
        let secs = FLY_SPEEDS[self.fly_speed.min(FLY_SPEEDS.len() - 1)].1;
        if secs <= 0.0 {
            self.cam = to;
            self.cam.normalize();
            self.fly = None;
            return;
        }
        self.fly = Some(Fly::new(self.cam, to, now, min_duration, secs));
    }

    fn goto_city(&mut self, lon: f64, lat: f64, now: f64) {
        self.goto_zoom(lon, lat, CITY_ZOOM, now);
    }

    /// Fly to a place (degrees) at a zoom level, keeping the tilt and rotation.
    fn goto_zoom(&mut self, lon: f64, lat: f64, zoom: f64, now: f64) {
        let alt = Camera::alt_for_zoom(zoom, lat.to_radians(), self.view_h);
        let mut to = Camera::from_degrees(lon, lat, alt);
        to.pitch = self.cam.pitch;
        to.heading = self.cam.heading;
        self.fly_to(to, now, 3.0);
    }

    fn reset_orientation(&mut self, now: f64) {
        let mut to = self.cam;
        to.reset_orientation();
        self.fly_to(to, now, 0.7);
    }

    fn look_at_plane(&mut self, i: usize, now: f64) {
        let p = &self.planes[i];
        let st = p.sim.state(&p.route);
        let (lon, lat) = vec_lon_lat(st.ground);
        let len = p.def.length_m * self.plane_size / flight::EARTH_RADIUS_M;
        let height = st.alt_m * self.exaggeration / flight::EARTH_RADIUS_M;
        let mut to = Camera { lon, lat, alt: (8.0 * len).max(3.0 * height), pitch: 0.9, heading: self.cam.heading, shift: [0.0, 0.0] };
        to.normalize();
        self.fly_to(to, now, 1.6);
    }

    fn step_fly(&mut self, now: f64) -> bool {
        let Some(f) = &self.fly else { return false };
        let t = ((now - f.start) / f.duration.max(1e-3)).clamp(0.0, 1.0);
        self.cam = if t >= 1.0 { f.to } else { f.camera(t) };
        self.cam.normalize();
        if t >= 1.0 {
            self.fly = None;
        }
        true
    }

    /// Multiplies the camera distance by `factor`, keeping the point under `cursor` fixed.
    fn zoom_at(&mut self, factor: f64, cursor: Option<egui::Pos2>, rect: egui::Rect) {
        let (w, h) = (rect.width() as f64, rect.height() as f64);
        let local = cursor.map(|c| (c.x as f64 - rect.min.x as f64, c.y as f64 - rect.min.y as f64));
        let before = local.and_then(|(x, y)| self.cam.pick(x, y, w, h));
        self.cam.alt = (self.cam.alt * factor).clamp(MIN_ALT, MAX_ALT);
        if let (Some(b), Some((x, y))) = (before, local) {
            if let Some(a) = self.cam.pick(x, y, w, h) {
                self.cam.lon += b.0 - a.0;
                self.cam.lat += b.1 - a.1;
            }
        }
        self.cam.normalize();
    }

    /// The small globe in the bottom-right corner (click it to put the map north-up and flat).
    fn compass(&mut self, ctx: &egui::Context, now: f64) {
        let tilt = self.cam.pitch.to_degrees();
        let (lon, lat, heading) = (self.cam.lon, self.cam.lat, self.cam.heading);
        let _ = now;
        // The picture is made again only when the view has turned (about a fifth of a degree).
        let size = 118.0_f32;
        let r = size * 0.5 - 6.0;
        let ppp = ctx.pixels_per_point();
        let key = [(lon * 300.0).round(), (lat * 300.0).round(), (heading * 300.0).round(), ppp as f64];
        if key != self.globe_key || self.globe_tex.is_none() {
            let dim = ((2.0 * (r + 1.0)) * ppp).ceil() as usize;
            let img = self.land.render(dim.clamp(32, 512), lon, lat, heading);
            match &mut self.globe_tex {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => self.globe_tex = Some(ctx.load_texture("mini-globe", img, egui::TextureOptions::LINEAR)),
            }
            self.globe_key = key;
        }
        let mut clicked = false;
        let tex = self.globe_tex.as_ref();
        egui::Area::new(egui::Id::new("compass")).anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-18.0, -18.0)).show(ctx, |ui| {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
            let p = ui.painter_at(rect.expand(8.0));
            globe::draw(&p, rect.center(), r, tex, lon, lat, heading);
            if tilt > 1.0 {
                p.text(
                    egui::pos2(rect.center().x, rect.max.y + 4.0),
                    egui::Align2::CENTER_BOTTOM,
                    format!("tilt {tilt:.0}°"),
                    egui::FontId::monospace(10.0),
                    egui::Color32::from_white_alpha(210),
                );
            }
            clicked = resp.on_hover_text("Where you are on the planet. Click: north up and straight down (R)").clicked();
        });
        if clicked {
            self.reset_orientation(now);
        }
    }
}

/// Zoom level at which a place `span_rad` radians across fills the view comfortably.
fn zoom_for_span(span_rad: f64, lat_rad: f64, view_h: f64) -> f64 {
    let span = (span_rad * 1.3).max(1e-6);
    (2.0 * std::f64::consts::PI * lat_rad.cos().max(0.05) * view_h / (512.0 * span)).log2().clamp(2.5, 17.5)
}

fn vec_lon_lat(p: V3) -> (f64, f64) {
    camera::vec_to_lonlat(p)
}

/// Size of a rectangle in physical pixels, rounded the way egui rounds its paint-callback viewport.
fn rect_px(rect: egui::Rect, ppp: f32) -> [u32; 2] {
    let w = (rect.max.x * ppp).round() - (rect.min.x * ppp).round();
    let h = (rect.max.y * ppp).round() - (rect.min.y * ppp).round();
    [w.max(1.0) as u32, h.max(1.0) as u32]
}

/// How long a moving plane can wait before redrawing: until it has crossed about a pixel
/// (a real-time flight in a wide view barely moves, so it needs very few frames).
fn plane_redraw_interval(p: &Plane, pr: &camera::Projector, speed: f64, min_s: f64) -> f64 {
    const PROBE_S: f64 = 0.25;
    let now = p.sim.state(&p.route);
    let mut later = p.sim;
    later.advance(PROBE_S, speed, p.route.duration_s());
    let later = later.state(&p.route);
    let (Some(a), Some(b)) = (pr.project(now.ground), pr.project(later.ground))
    else {
        return 0.25;
    };
    let px_per_s = ((a[0] - b[0]).hypot(a[1] - b[1]) / PROBE_S).max(1e-3);
    (1.0 / px_per_s).max(min_s).min(0.25)
}

/// Wakes the UI at a steady rate while something is animating. egui's own `request_repaint_after`
/// proved less steady than a plain timer thread (the 30 and 60 fps settings both ran at 60).
struct Pacer {
    interval_ms: Arc<std::sync::atomic::AtomicU64>,
}

impl Pacer {
    fn new(ctx: &egui::Context) -> Self {
        use std::sync::atomic::Ordering::Relaxed;
        let interval_ms = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (iv, ctx) = (interval_ms.clone(), ctx.clone());
        let _ = std::thread::Builder::new().name("pacer".into()).spawn(move || loop {
            let ms = iv.load(Relaxed);
            std::thread::sleep(std::time::Duration::from_millis(if ms == 0 { 40 } else { ms }));
            // Re-read: the UI may have switched the animation off while we slept.
            if ms > 0 && iv.load(Relaxed) > 0 {
                ctx.request_repaint();
            }
        });
        Pacer { interval_ms }
    }

    /// Seconds until the next animation frame is needed, or infinity when nothing is animating.
    fn set(&self, secs: f64) {
        let ms = if secs.is_finite() { (secs * 1000.0).round().max(1.0) as u64 } else { 0 };
        self.interval_ms.store(ms, std::sync::atomic::Ordering::Relaxed);
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = ctx.input(|i| i.time);
        let dt = ctx.input(|i| i.stable_dt) as f64;
        if dt > 0.0 {
            self.fps = self.fps * 0.9 + (1.0 / dt) * 0.1;
        }
        // Once a second: how many frames were drawn, and how many of them really redrew the map / planes.
        {
            use std::sync::atomic::Ordering::Relaxed;
            self.rate_since.1 += 1;
            if ctx.input(|i| !i.events.is_empty() || i.pointer.delta() != egui::Vec2::ZERO) {
                self.input_frames += 1;
            }
            if now - self.rate_since.0 >= 1.0 {
                let (m, pl) = (gpu::MAP_REDRAWS.load(Relaxed), plane::PLANE_REDRAWS.load(Relaxed));
                let span = (now - self.rate_since.0).max(0.001);
                let per = |n: u64| (n as f64 / span).round() as u32;
                self.rates = [per(self.rate_since.1 as u64), per(m - self.rate_since.2), per(pl - self.rate_since.3)];
                self.input_rate = (self.input_frames as f64 / span).round() as u32;
                self.input_frames = 0;
                self.rate_since = (now, 0, m, pl);
            }
        }
        self.transit.poll();
        self.tracks.poll();
        if self.show_tracks && self.track_view == tracks::View::Snapped {
            self.tracks.request_snap();
        }
        if let Some(s) = &self.phone_sync {
            for f in s.take_saved() {
                self.tracks.reload_file(&f);
            }
        }
        self.search.poll();
        // A .kml file dropped on the window becomes a GPS track.
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        for path in dropped {
            self.tracks.import(&path);
            self.layers_open = true;
            self.show_tracks = true;
        }

        let speed = SPEEDS[self.speed].1;
        // Flights advance by the real time since the previous frame, however far apart frames are.
        let anim_dt = if self.last_anim > 0.0 { (now - self.last_anim).clamp(0.0, 0.5) } else { 0.0 };
        self.last_anim = now;
        // The fastest the animation may redraw (the "animation rate" setting); trains never need more than 30.
        let frame = std::time::Duration::from_millis(1000 / self.anim_fps.max(1) as u64);
        // Only planes that are switched on count: a hidden plane must not keep the screen redrawing.
        // How often they redraw is decided below, from how far they actually move on screen.
        for p in &mut self.planes {
            if p.show && p.sim.playing {
                p.sim.advance(anim_dt, speed, p.route.duration_s());
            }
        }
        if self.step_fly(now) {
            ctx.request_repaint();
        } else if let Some(i) = self.follow {
            let p = &self.planes[i];
            if p.show {
                let (lon, lat) = vec_lon_lat(p.sim.state(&p.route).ground);
                self.cam.lon = lon;
                self.cam.lat = lat;
                self.cam.normalize();
            }
        }
        // A GPS path being replayed: its clock runs, and the map can follow the walker.
        if let Some(pl) = &mut self.player {
            if let Some(it) = self.tracks.items.get(pl.item) {
                let tl = it.track.timeline(self.track_view);
                pl.advance(anim_dt, tl.total_secs());
                if pl.playing {
                    ctx.request_repaint();
                    if pl.follow && self.fly.is_none() {
                        let (lon, lat) = vec_lon_lat(tl.at(pl.u).pos);
                        self.cam.lon = lon;
                        self.cam.lat = lat;
                        self.cam.normalize();
                    }
                }
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F11)) {
            let on = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!on));
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.sign = None;
            if ctx.input(|i| i.viewport().fullscreen.unwrap_or(false)) && self.tab.is_none() && self.selected_line.is_none() && !self.layers_open {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            }
            self.selected_line = None;
            self.player = None;
            self.tab = None;
            self.layers_open = false;
            self.search_open = false;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::R)) && !ctx.wants_keyboard_input() {
            self.reset_orientation(now);
        }

        let mut anim_wait = f64::INFINITY; // seconds until the next animation frame is needed
        let mut rect_for_ui = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0));
        let mut zf_for_ui = 0.0;

        egui::CentralPanel::default().frame(egui::Frame::new()).show(ctx, |ui| {
            let rect = ui.max_rect();
            rect_for_ui = rect;
            let resp = ui.allocate_rect(rect, egui::Sense::click_and_drag());
            let (w, h) = (rect.width() as f64, rect.height() as f64);
            if w < 8.0 || h < 8.0 {
                return;
            }
            self.view_h = h;

            // --- input ---
            let d = resp.drag_delta();
            if d != egui::Vec2::ZERO {
                if resp.dragged_by(egui::PointerButton::Primary) {
                    self.fly = None;
                    if ctx.input(|i| i.modifiers.shift || i.modifiers.ctrl) {
                        // Shift/Ctrl + drag: slide the whole globe around the window.
                        self.cam.shift_by(d.x as f64, d.y as f64, w, h);
                    } else {
                        self.follow = None;
                        if let Some(pl) = &mut self.player {
                            pl.follow = false;
                        }
                        self.cam.pan(d.x as f64, d.y as f64, h);
                    }
                } else if resp.dragged_by(egui::PointerButton::Secondary) || resp.dragged_by(egui::PointerButton::Middle) {
                    self.fly = None;
                    self.cam.orbit(d.x as f64, d.y as f64);
                }
            }
            if resp.hovered() {
                let (scroll, pinch) = ctx.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
                let factor = (-(scroll as f64) * 0.003).exp() / pinch as f64;
                if (factor - 1.0).abs() > 1e-6 {
                    self.fly = None;
                    self.zoom_at(factor, resp.hover_pos(), rect);
                }
            }
            if resp.double_clicked() {
                if let Some(p) = resp.interact_pointer_pos() {
                    if let Some((lon, lat)) = self.cam.pick((p.x - rect.min.x) as f64, (p.y - rect.min.y) as f64, w, h) {
                        let mut to = self.cam;
                        to.lon = lon;
                        to.lat = lat;
                        to.alt = self.cam.alt * 0.35;
                        to.normalize();
                        self.fly_to(to, now, 0.9);
                    }
                }
            }

            // --- tiles ---
            let zf = self.cam.zoom_f(h);
            zf_for_ui = zf;
            let frame_tiles = self.tiles.update(&self.cam, w, h);
            self.stats = Stats {
                drawn: frame_tiles.draw.len(),
                loaded: frame_tiles.loaded,
                pending: frame_tiles.pending,
                failed: frame_tiles.failed,
            };

            // --- map (GPU) ---
            let ppp = ctx.pixels_per_point();
            let pos = self.cam.pos();
            let globals = Globals {
                view_proj: self.cam.view_proj_cols(w / h),
                proj: self.cam.rel_proj_cols(w / h),
                cam_pos: [pos[0] as f32, pos[1] as f32, pos[2] as f32, if self.srgb { 1.0 } else { 0.0 }],
                cam_rel: [
                    self.cam.lat.cos() as f32,
                    self.cam.lat.sin() as f32,
                    (self.cam.alt * self.cam.pitch.cos()) as f32,
                    (self.cam.alt * self.cam.pitch.sin()) as f32,
                ],
                head: [self.cam.heading.sin() as f32, self.cam.heading.cos() as f32, 0.0, 0.0],
                screen: [w as f32 * ppp, h as f32 * ppp, 0.0, 0.0],
                styles: style::table(zf, ppp, self.palette, &self.detail),
            };
            let painter = ui.painter().with_clip_rect(rect);
            painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(8, 10, 22));
            painter.add(egui_wgpu::Callback::new_paint_callback(
                rect,
                MapCallback {
                    globals,
                    cam_merc: camera::lonlat_to_merc(self.cam.lon, self.cam.lat),
                    draw: frame_tiles.draw.clone(),
                    viewport_px: rect_px(rect, ppp),
                },
            ));

            // --- overlays (egui), bottom to top: routes, transit, plane drop lines ---
            let pr = self.cam.projector(w, h);
            let mut extras: Vec<Cand> = Vec::new();
            let rs = RouteStyle {
                color: self.route_color,
                alpha: self.route_alpha,
                width: self.route_width,
                at_altitude: self.route_at_altitude,
                exaggeration: self.exaggeration,
            };
            // Layers: with the transit map switched off nothing of it is drawn or clickable.
            let systems = if self.show_transit { self.transit.visible() } else { Vec::new() };
            let line_prefs = if self.show_transit { self.transit.visible_prefs() } else { Vec::new() };
            // With a transit map in view the base map steps back: dimmed, and its labels hidden.
            let focus = if self.transit_focus { transit::focus(&self.cam, zf, w / h, &systems) } else { 0.0 };
            if focus > 0.01 && self.map_dim > 0.0 {
                let a = (focus * self.map_dim * 255.0).round() as u8;
                painter.rect_filled(rect, 0.0, egui::Color32::from_rgba_unmultiplied(12, 14, 22, a));
            }
            draw_routes(&painter, rect, &pr, &self.planes, &rs, &mut extras);
            // A click may hit a metro symbol (fly to that city) or a subway line (highlight it).
            let click = if resp.clicked() { resp.interact_pointer_pos() } else { None };
            let markers = if self.show_transit { self.transit.markers() } else { Vec::new() };
            let marker_click = click.and_then(|p| transit::marker_at(&self.cam, rect, zf, &markers, p));
            if let Some(p) = resp.hover_pos() {
                if transit::marker_at(&self.cam, rect, zf, &markers, p).is_some() {
                    ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                }
            }
            let out = {
                let opts = transit::Opts {
                    flat_stations: self.flat_stations,
                    trains: self.show_trains,
                    now,
                    train_speed: TRAIN_SPEEDS[self.train_speed].1,
                    pick: if marker_click.is_none() { click } else { None },
                    selected: self.selected_line.as_ref().map(|(a, i)| (a, *i)),
                    bilingual: self.bilingual,
                    prefs: &line_prefs,
                };
                transit::draw(&painter, rect, &self.cam, zf, &systems, &opts)
            };
            // GPS paths from the phone tracker, over the transit lines and under the names.
            if self.show_tracks {
                let sel = self.player.as_ref().map(|p| p.item);
                let found = tracks::draw(&painter, rect, &self.cam, zf, &self.tracks.items, self.track_view, sel, resp.hover_pos(), click);
                if found.hover.is_some() {
                    ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if let Some(i) = found.click {
                    if sel != Some(i) {
                        let total = self.tracks.items[i].track.timeline(self.track_view).total_secs();
                        self.player = Some(tracks::Player::new(i, total));
                    }
                }
                if let Some(pl) = &self.player {
                    if let Some(it) = self.tracks.items.get(pl.item) {
                        if it.shown {
                            tracks::draw_marker(&painter, rect, &self.cam, it.track.timeline(self.track_view).at(pl.u).pos, it.track.color, now);
                        }
                    }
                }
                if let Some(text) = found.caption {
                    let galley = painter.layout_no_wrap(text, egui::FontId::proportional(14.0), egui::Color32::WHITE);
                    let at = resp.hover_pos().unwrap_or(rect.center()) + egui::vec2(14.0, 16.0);
                    let r = egui::Rect::from_min_size(at, galley.size()).expand2(egui::vec2(8.0, 4.0));
                    painter.rect_filled(r, 6.0, egui::Color32::from_black_alpha(200));
                    painter.galley(r.min + egui::vec2(8.0, 4.0), galley, egui::Color32::WHITE);
                }
            }
            if out.animating {
                // Redraw about once per pixel the fastest train crosses (slow real-time trains need very few frames).
                let px_s = (out.motion_px_s as f64).max(1e-3);
                anim_wait = anim_wait.min((1.0 / px_s).max(frame.as_secs_f64()).min(0.25));
            }
            let line_hit = out.hit;
            for l in out.labels {
                extras.push(Cand::from_station(l));
            }
            // A station under the pointer: its platform sign shows while hovering, and stays when clicked.
            let near_station = |p: egui::Pos2| -> Option<(usize, usize)> {
                out.shown
                    .iter()
                    .filter(|s| s.at.distance(p) <= s.r + 6.0)
                    .min_by(|a, b| a.at.distance(p).partial_cmp(&b.at.distance(p)).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|s| (s.sys, s.st))
            };
            let station_click = click.and_then(near_station);
            let hover_station = resp.hover_pos().and_then(near_station);
            if hover_station.is_some() {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if let Some((si, sti)) = station_click {
                let same = matches!(&self.sign, Some(g) if g.pinned && Arc::ptr_eq(&g.sys, &systems[si]) && g.st == sti);
                if same {
                    if let Some(g) = &mut self.sign {
                        g.line += 1;
                    }
                } else {
                    self.sign = Some(Sign { sys: systems[si].clone(), st: sti, line: 0, pinned: true });
                }
            }
            if let Some(mi) = marker_click {
                let (lat, lon) = markers[mi].home;
                self.goto_city(lon, lat, now);
            } else if station_click.is_some() {
                // the sign is the answer to this click; leave the line selection alone
            } else if click.is_some() {
                if line_hit.is_none() {
                    self.tab = None;
                    self.search_open = false;
                    self.sign = None;
                }
                self.selected_line = line_hit;
            }
            // Hovering shows a sign too (unless one is pinned); it goes away when the pointer leaves.
            let hover_sign = match (hover_station, &self.sign) {
                (Some((si, sti)), s) if !matches!(s, Some(g) if g.pinned) => {
                    let line = self.selected_line.as_ref().and_then(|(a, li)| {
                        Arc::ptr_eq(a, &systems[si]).then(|| systems[si].stations[sti].lines.iter().position(|l| l == li)).flatten()
                    });
                    Some((si, sti, line.unwrap_or(0)))
                }
                _ => None,
            };
            let mut draws: Vec<PlaneDraw> = Vec::new();
            let mut bounds: Option<[f32; 4]> = None; // screen box around all planes (NDC)
            let mut whole_screen = false;
            let mut wait = 0.25_f64; // seconds until the planes next need a redraw
            for (i, p) in self.planes.iter().enumerate() {
                let (true, Some(radius)) = (p.show, p.radius_m) else { continue };
                let st = p.sim.state(&p.route);
                draw_plane_marker(&painter, rect, &self.cam, &pr, &st, p.def, self.plane_size, self.exaggeration);
                if p.sim.playing {
                    wait = wait.min(plane_redraw_interval(p, &pr, speed, frame.as_secs_f64()));
                }
                if let Some((mvp, nrm)) = flight::plane_matrices(&self.cam, w / h, &st, self.plane_size, self.exaggeration, radius) {
                    match flight::ndc_bounds(&mvp, radius) {
                        Some(b) => {
                            bounds = Some(match bounds {
                                Some(a) => [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])],
                                None => b,
                            })
                        }
                        None => whole_screen = true,
                    }
                    draws.push(PlaneDraw { model: i, mvp, nrm });
                }
            }
            if self.planes.iter().any(|p| p.show && p.sim.playing) {
                anim_wait = anim_wait.min(wait.max(frame.as_secs_f64()));
            }

            // --- labels ---
            if self.show_labels {
                let ls = LabelStyle { scale: LABEL_SIZES[self.label_size].1, bold: self.label_bold, dark: self.palette.dark(), family: fonts::Fonts::family(), real_bold: self.fonts.has_real_bold(), detail: self.detail.clone() };
                let map_labels = if focus > 0.5 { &frame_tiles.labels[..0] } else { &frame_tiles.labels[..] };
                let blocked = transit::marker_rects(&self.cam, rect, zf, &markers);
                draw_labels(&painter, rect, &pr, zf, map_labels, extras, &ls, &blocked, &mut self.labels_shown);
            }
            // The metro symbols sit on top of the labels, then the aircraft on top of both.
            transit::draw_markers(&painter, rect, &self.cam, zf, &markers);
            // --- the aircraft themselves: above everything on the map, labels included ---
            // Only the part of the screen that holds a plane is rendered and composited.
            let vp = rect_px(rect, ppp);
            if let Some(bbox) = flight::plane_box(bounds, whole_screen, vp) {
                for d in &mut draws {
                    flight::remap_to_box(&mut d.mvp, vp, bbox);
                }
                painter.add(egui_wgpu::Callback::new_paint_callback(
                    rect,
                    PlaneCallback { draws, srgb: self.srgb, bbox, supersample: self.plane_quality == 0 },
                ));
            }

            // --- the platform sign of a station: on top of the map, the labels and the aircraft ---
            if let Some((si, sti, line)) = hover_sign {
                if let Some(at) = out.shown.iter().find(|s| s.sys == si && s.st == sti).map(|s| s.at) {
                    transit::draw_sign(&painter, rect, at - egui::vec2(0.0, 6.0), &systems[si], sti, line);
                }
            } else if let Some(g) = &self.sign {
                // pinned: find where its station is on screen now (it may have moved or gone)
                let shown = systems.iter().position(|s| Arc::ptr_eq(s, &g.sys)).and_then(|si| out.shown.iter().find(|s| s.sys == si && s.st == g.st));
                if let Some(s) = shown {
                    transit::draw_sign(&painter, rect, s.at - egui::vec2(0.0, 6.0), &g.sys, g.st, g.line);
                }
            }

            // --- name of the highlighted subway line ---
            if let Some((sys, li)) = &self.selected_line {
                if let Some(line) = sys.lines.get(*li) {
                    let text = if line.badge.is_empty() { line.name.clone() } else { format!("{}   {}", line.badge, line.name) };
                    let galley = painter.layout_no_wrap(text, egui::FontId::proportional(15.0), egui::Color32::WHITE);
                    let r = egui::Rect::from_center_size(rect.center_bottom() - egui::vec2(0.0, 44.0), galley.size()).expand2(egui::vec2(12.0, 6.0));
                    let c = line.color;
                    painter.rect_filled(r, 14.0, egui::Color32::from_rgb(c[0] / 2, c[1] / 2, c[2] / 2));
                    painter.rect_stroke(r, 14.0, egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(c[0], c[1], c[2])), egui::StrokeKind::Inside);
                    painter.galley(r.center() - galley.size() * 0.5, galley, egui::Color32::WHITE);
                }
            }

            // --- attribution (required by the map data licences) ---
            egui::Area::new(egui::Id::new("attribution")).anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(8.0, -8.0)).show(ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_black_alpha(170))
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(6, 3))
                    .show(ui, |ui| {
                        ui.set_max_width((rect.width() - 130.0).max(200.0));
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.hyperlink_to("OpenFreeMap", "https://openfreemap.org");
                            ui.hyperlink_to("© OpenMapTiles", "https://openmaptiles.org/");
                            ui.label("Data from");
                            ui.hyperlink_to("OpenStreetMap", "https://www.openstreetmap.org/copyright");
                        });
                    });
            });
        });

        // While the view moves, text may sit between pixels so it glides with the map instead of
        // stepping; at rest it snaps to the pixel grid for crispness.
        let cam_now = [self.cam.lon, self.cam.lat, self.cam.alt, self.cam.pitch, self.cam.heading, self.cam.shift[0], self.cam.shift[1]];
        let moving = cam_now != self.prev_cam;
        self.prev_cam = cam_now;
        ctx.tessellation_options_mut(|o| o.round_text_to_pixels = !moving);
        if self.player.as_ref().map_or(false, |p| p.playing) {
            anim_wait = anim_wait.min(frame.as_secs_f64());
        }
        self.pacer.set(anim_wait);
        if self.show_compass {
            self.compass(ctx, now);
        }
        self.controls(ctx, rect_for_ui, now, zf_for_ui);
        self.systems_popup(ctx, rect_for_ui, zf_for_ui);
        if !self.show_tracks {
            self.player = None;
        }
        let mut close_card = false;
        if let Some(pl) = &mut self.player {
            close_card = trackinfo::card(ctx, &self.tracks.items, self.track_view, pl);
        }
        if close_card {
            self.player = None;
        }

        if now - self.saved_at > 1.0 {
            self.saved_at = now;
            let text = self.settings_text();
            if text != self.saved_settings {
                let path = settings::path();
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if std::fs::write(&path, &text).is_ok() {
                    self.saved_settings = text;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Overlays
// ---------------------------------------------------------------------------

fn to_screen(pr: &Projector, rect: egui::Rect, p: V3) -> Option<egui::Pos2> {
    if !pr.visible(p) {
        return None;
    }
    pr.project(p).map(|s| rect.min + egui::vec2(s[0] as f32, s[1] as f32))
}

/// Route lines and airport markers of every shown aircraft (the planes are drawn above these).
fn draw_routes(painter: &egui::Painter, rect: egui::Rect, pr: &Projector, planes: &[Plane], rs: &RouteStyle, labels: &mut Vec<Cand>) {
    let mut airports: Vec<&str> = Vec::new();
    for p in planes.iter().filter(|p| p.show) {
        let mut run: Vec<egui::Pos2> = Vec::new();
        let mut runs: Vec<Vec<egui::Pos2>> = Vec::new();
        let line = if rs.at_altitude { p.route.polyline_alt(360, rs.exaggeration) } else { p.route.polyline(360) };
        for v in line {
            match to_screen(pr, rect, v) {
                Some(s) => run.push(s),
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
        let c = rs.color;
        let a = (rs.alpha.clamp(0.0, 1.0) * 255.0) as u8;
        let line = egui::Color32::from_rgba_unmultiplied(c[0], c[1], c[2], a);
        let casing = egui::Color32::from_rgba_unmultiplied(255, 255, 255, (a as f32 * 0.45) as u8);
        for r in &runs {
            painter.add(egui::Shape::line(r.clone(), egui::Stroke::new(rs.width + 2.0, casing)));
        }
        for r in &runs {
            painter.add(egui::Shape::line(r.clone(), egui::Stroke::new(rs.width, line)));
        }
        for (name, lon, lat) in [p.def.from, p.def.to] {
            if airports.contains(&name) {
                continue;
            }
            airports.push(name);
            let v = camera::lonlat_to_vec(lon.to_radians(), lat.to_radians());
            if let Some(s) = to_screen(pr, rect, v) {
                painter.circle(s, 4.0, egui::Color32::WHITE, egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(c[0], c[1], c[2])));
                labels.push(Cand::new(0.5, name.to_string(), 12.0, [c[0] / 3, c[1] / 3, c[2] / 3], [255, 255, 255], s + egui::vec2(0.0, -14.0)));
            }
        }
    }
}

/// A line from the plane down to the spot on the ground beneath it, a dot there, and a ring that
/// keeps a tiny plane findable from afar.
#[allow(clippy::too_many_arguments)]
fn draw_plane_marker(
    painter: &egui::Painter,
    rect: egui::Rect,
    cam: &Camera,
    pr: &Projector,
    st: &flight::FlightState,
    def: &AircraftDef,
    size_mul: f64,
    exaggeration: f64,
) {
    let (pos, _r, fwd, _u) = flight::plane_frame(st, exaggeration);
    let c = def.color;
    let col = egui::Color32::from_rgb(c[0], c[1], c[2]);
    let ground = to_screen(pr, rect, st.ground);
    let plane = if occluded(cam, pos) { None } else { pr.project(pos).map(|s| rect.min + egui::vec2(s[0] as f32, s[1] as f32)) };
    if let (Some(g), Some(p)) = (ground, plane) {
        if g.distance(p) > 3.0 {
            painter.line_segment([g, p], egui::Stroke::new(1.2_f32, egui::Color32::from_white_alpha(170)));
        }
        painter.circle(g, 2.5, col, egui::Stroke::new(1.0_f32, egui::Color32::WHITE));
    }
    // Ring around the plane while it is smaller than ~14 px.
    let nose = camera_add(pos, fwd, 0.5 * def.length_m * size_mul / flight::EARTH_RADIUS_M);
    if let (Some(a), Some(b)) = (pr.project(pos), pr.project(nose)) {
        if (a[0] - b[0]).hypot(a[1] - b[1]) < 7.0 && !occluded(cam, pos) {
            let ctr = rect.min + egui::vec2(a[0] as f32, a[1] as f32);
            if rect.contains(ctr) {
                painter.circle_stroke(ctr, 11.0, egui::Stroke::new(2.0_f32, col));
            }
        }
    }
}

fn camera_add(p: V3, d: V3, s: f64) -> V3 {
    [p[0] + d[0] * s, p[1] + d[1] * s, p[2] + d[2] * s]
}

/// True when the planet is between the camera and `p`.
fn occluded(cam: &Camera, p: V3) -> bool {
    let c = cam.pos();
    let rel = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
    let t = (-dot(c, rel) / dot(rel, rel)).clamp(0.0, 1.0);
    let q = camera_add(c, rel, t);
    dot(q, q) < 1.0 - 1e-9
}

/// A label waiting for a spot on screen.
struct Cand {
    priority: f32,
    text: String,
    size: f32,
    color: [u8; 3],
    halo: [u8; 3],
    at: egui::Pos2,
    /// Text rotation in radians (street names run along their road); 0 for everything else.
    angle: f32,
    /// Identity across frames (hash of the text), so labels shown last frame keep their place.
    key: u64,
    /// A smaller second line under the text (the English name under a local one); "" for none.
    sub: String,
    /// A coloured dot at `at` with the text underneath it (places of interest).
    dot: Option<[u8; 3]>,
}

impl Cand {
    fn new(priority: f32, text: String, size: f32, color: [u8; 3], halo: [u8; 3], at: egui::Pos2) -> Self {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        Cand { priority, text, size, color, halo, at, angle: 0.0, key: h.finish(), sub: String::new(), dot: None }
    }

    fn from_station(l: StationLabel) -> Self {
        let priority = if l.interchange { 9.5 } else { 17.0 };
        let mut c = Cand::new(priority, l.text, if l.interchange { 12.5 } else { 11.5 }, [20, 20, 30], [255, 255, 255], l.at);
        c.sub = l.sub;
        c
    }
}

/// How labels look: size multiplier, bold, and whether the map is dark.
struct LabelStyle {
    scale: f32,
    bold: bool,
    dark: bool,
    family: egui::FontFamily,
    /// The chosen font has an installed bold face, so bold uses it instead of overdrawing.
    real_bold: bool,
    /// Which kinds of label are switched off.
    detail: detail::MapDetail,
}

/// Places labels greedily by priority; a label that would overlap one already placed is dropped.
fn draw_labels(
    painter: &egui::Painter,
    rect: egui::Rect,
    pr: &Projector,
    zf: f64,
    tiles: &[Arc<Vec<mesh::Label>>],
    mut cands: Vec<Cand>,
    ls: &LabelStyle,
    blocked: &[egui::Rect],
    shown: &mut std::collections::HashSet<u64>,
) {
    let (w, h) = (rect.width() as f64, rect.height() as f64);
    for t in tiles {
        for l in t.iter() {
            if !ls.detail.label_on(l.kind, l.cat) {
                continue;
            }
            let Some(rule) = style::label_rule(l.kind, l.rank, zf, ls.dark) else { continue };
            let p = merc_to_vec(l.mx, l.my);
            if !pr.visible(p) {
                continue;
            }
            let Some(s) = pr.project(p) else { continue };
            if s[0] < -40.0 || s[0] > w + 40.0 || s[1] < -20.0 || s[1] > h + 20.0 {
                continue;
            }
            let text = if rule.upper { l.text.to_uppercase() } else { l.text.clone() };
            let at = rect.min + egui::vec2(s[0] as f32, s[1] as f32);
            let mut cand = Cand::new(rule.priority, text, rule.size, rule.color, rule.halo, at);
            if l.kind == mesh::Kind::Poi {
                cand.dot = Some(detail::poi_color(l.cat));
            }
            if l.kind == mesh::Kind::Peak {
                cand.dot = Some([150, 100, 60]);
            }
            if l.kind == mesh::Kind::Street {
                // Only when the road piece is long enough for its name, and turned along the road
                // (never upside down).
                let world_px = 512.0 * 2f64.powf(zf);
                if (l.span as f64 * world_px) < cand.text.chars().count() as f64 * rule.size as f64 * ls.scale as f64 * 0.55 {
                    continue;
                }
                let d = 24.0 / world_px;
                let Some(s2) = pr.project(merc_to_vec(l.mx + l.dir[0] as f64 * d, l.my + l.dir[1] as f64 * d)) else { continue };
                let mut a = (s2[1] - s[1]).atan2(s2[0] - s[0]);
                if a > std::f64::consts::FRAC_PI_2 {
                    a -= std::f64::consts::PI;
                } else if a <= -std::f64::consts::FRAC_PI_2 {
                    a += std::f64::consts::PI;
                }
                cand.angle = a as f32;
            }
            cands.push(cand);
        }
    }
    // A label that was on screen last frame wins ties, so labels do not flicker in and out while
    // the map moves.
    for c in &mut cands {
        if shown.contains(&c.key) {
            c.priority -= 0.4;
        }
    }
    cands.sort_by(|a, b| a.priority.partial_cmp(&b.priority).unwrap_or(std::cmp::Ordering::Equal));
    shown.clear();

    let mut placed: Vec<egui::Rect> = blocked.to_vec();
    let fake_bold = ls.bold && !ls.real_bold;
    let bold = if fake_bold { 0.45_f32 } else { 0.0 };
    let family = if ls.bold && ls.real_bold { fonts::Fonts::bold_family() } else { ls.family.clone() };
    for c in cands.into_iter().take(700) {
        if placed.len() >= 220 + blocked.len() {
            break;
        }
        let font = egui::FontId::new(c.size * ls.scale, family.clone());
        let col = egui::Color32::from_rgb(c.color[0], c.color[1], c.color[2]);
        let halo = egui::Color32::from_rgb(c.halo[0], c.halo[1], c.halo[2]);
        let main = painter.layout_no_wrap(c.text.clone(), font.clone(), col);
        let (sn, cs) = c.angle.sin_cos();
        let size = main.size();
        // an optional smaller second line (English under a local name); never on turned text
        let sub = (c.angle == 0.0 && !c.sub.is_empty())
            .then(|| painter.layout_no_wrap(c.sub.clone(), egui::FontId::new(c.size * ls.scale * 0.84, ls.family.clone()), col));
        let sub_size = sub.as_ref().map_or(egui::Vec2::ZERO, |g| g.size());
        // box around the (possibly turned) text, for collisions
        let ext = if sub.is_some() {
            egui::vec2(size.x.max(sub_size.x), size.y + sub_size.y)
        } else {
            egui::vec2(size.x * cs.abs() + size.y * sn.abs(), size.x * sn.abs() + size.y * cs.abs())
        };
        // a dot sits above its text
        let text_centre = if c.dot.is_some() { c.at + egui::vec2(0.0, 7.0 + size.y * 0.5) } else { c.at };
        let r = egui::Rect::from_center_size(text_centre, ext);
        let mut padded = r.expand2(egui::vec2(4.0, 2.0));
        if c.dot.is_some() {
            padded = padded.union(egui::Rect::from_center_size(c.at, egui::vec2(10.0, 10.0)));
        }
        if placed.iter().any(|p| p.intersects(padded)) {
            continue;
        }
        placed.push(padded);
        shown.insert(c.key);
        if let Some(d) = c.dot {
            painter.circle(c.at, 4.0, egui::Color32::from_rgb(d[0], d[1], d[2]), egui::Stroke::new(1.6_f32, egui::Color32::WHITE));
        }

        let shadow = painter.layout_no_wrap(c.text, font, halo);
        if c.angle != 0.0 {
            // turned text: top-left corner chosen so the text is centred on its anchor
            let half = size * 0.5;
            let tl = c.at - egui::vec2(half.x * cs - half.y * sn, half.x * sn + half.y * cs);
            for (dx, dy) in [(-1.3_f32, 0.0_f32), (1.3, 0.0), (0.0, -1.3), (0.0, 1.3), (-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                painter.add(egui::epaint::TextShape::new(tl + egui::vec2(dx, dy), shadow.clone(), halo).with_angle(c.angle));
            }
            painter.add(egui::epaint::TextShape::new(tl, main, col).with_angle(c.angle));
            continue;
        }
        let o = 1.3 + bold * 1.4;
        let main_at = r.min + egui::vec2((ext.x - size.x) * 0.5, 0.0);
        for (dx, dy) in [(-o, 0.0), (o, 0.0), (0.0, -o), (0.0, o), (-o, -o), (o, -o), (-o, o), (o, o)] {
            painter.galley(main_at + egui::vec2(dx, dy), shadow.clone(), halo);
        }
        if fake_bold {
            // No bold face is installed: fatten the strokes by drawing the text a little off-centre.
            for (dx, dy) in [(-bold, 0.0), (bold, 0.0), (0.0, -bold * 0.6), (0.0, bold * 0.6)] {
                painter.galley(main_at + egui::vec2(dx, dy), main.clone(), col);
            }
        }
        painter.galley(main_at, main, col);
        if let Some(g) = sub {
            let sub_at = r.min + egui::vec2((ext.x - sub_size.x) * 0.5, size.y);
            let sub_shadow = painter.layout_no_wrap(c.sub.clone(), egui::FontId::new(c.size * ls.scale * 0.84, ls.family.clone()), halo);
            for (dx, dy) in [(-1.2_f32, 0.0_f32), (1.2, 0.0), (0.0, -1.2), (0.0, 1.2), (-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                painter.galley(sub_at + egui::vec2(dx, dy), sub_shadow.clone(), halo);
            }
            painter.galley(sub_at, g, col);
        }
    }
}
