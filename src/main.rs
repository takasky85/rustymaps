//! RustyMaps: a small globe-to-street-level vector map viewer.
//!
//! Rust + egui + wgpu. Tiles come from OpenFreeMap (vector, OpenMapTiles schema),
//! are drawn on the GPU by a custom paint callback, and labels are drawn by egui.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod camera;
mod data;
mod flight;
mod fonts;
mod gpu;
mod json;
mod mesh;
mod mvt;
mod plane;
mod search;
mod settings;
mod style;
mod tile_id;
mod tiles;
mod transit;
mod transit_data;
mod ui;

use std::sync::Arc;

use eframe::{egui, egui_wgpu, wgpu};

use camera::{dot, merc_to_vec, Camera, Projector, MAX_ALT, MIN_ALT, V3};
use flight::{AircraftDef, Route, Sim, AIRCRAFT};
use gpu::{Globals, MapCallback};
use plane::{PlaneCallback, PlaneDraw};
use style::Palette;
use tiles::TileManager;
use transit::{StationLabel, TransitManager};
use transit_data::{SystemData, SYSTEMS};

const SF_LON: f64 = -122.4194;
const SF_LAT: f64 = 37.7749;
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
    Ok(best)
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--fetch-transit") {
        let dir = args.get(i + 1).map(String::as_str).unwrap_or("transit");
        let only: Vec<String> = args.iter().skip(i + 2).cloned().collect();
        transit::fetch_to_dir(std::path::Path::new(dir), &only);
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]).with_title("RustyMaps"),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: egui_wgpu::WgpuConfiguration {
            wgpu_setup: egui_wgpu::WgpuSetup::CreateNew(egui_wgpu::WgpuSetupCreateNew {
                power_preference: wgpu::PowerPreference::HighPerformance,
                native_adapter_selector: Some(Arc::new(select_adapter)),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    eframe::run_native("RustyMaps", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}

/// A smooth camera move between two views. Long trips climb towards a globe view in the middle and
/// settle back down at the destination.
struct Fly {
    from: Camera,
    to: Camera,
    start: f64,
    duration: f64,
    /// Highest altitude (planet radii) reached on the way.
    peak: f64,
}

/// Playback speeds for the simulated flights (multiples of real time).
const SPEEDS: [(&str, f64); 4] = [("real time", 1.0), ("60x", 60.0), ("600x", 600.0), ("3600x", 3600.0)];

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

struct App {
    cam: Camera,
    tiles: TileManager,
    transit: TransitManager,
    palette: Palette,
    fly: Option<Fly>,
    srgb: bool,
    gpu_name: String,
    fps: f64,
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
    /// Frame rate of animations (planes, trains) when nothing else is happening.
    anim_fps: u32,
    /// Time of the previous frame, for advancing the flights by real elapsed time.
    last_anim: f64,
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

        let mut app = App {
            cam: Camera::from_degrees(-115.0, 32.0, 2.2),
            transit,
            tiles,
            palette: Palette::STANDARD,
            fly: None,
            srgb,
            gpu_name,
            fps: 60.0,
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
            anim_fps: 60,
            last_anim: 0.0,
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
        app.saved_settings = app.settings_text();
        app
    }

    /// Smooth move to `to`. Far destinations zoom out towards a globe view first and zoom back in
    /// as the destination gets close; `min_duration` is for short hops.
    fn fly_to(&mut self, to: Camera, now: f64, min_duration: f64) {
        self.follow = None;
        let dist = dot(self.cam.dir(), to.dir()).clamp(-1.0, 1.0).acos();
        let hi = self.cam.alt.max(to.alt);
        let peak = (dist * 4.0).min(2.4).max(hi);
        let duration = (min_duration + dist * 2.5).min(9.0);
        self.fly = Some(Fly { from: self.cam, to, start: now, duration, peak });
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
        let t = ((now - f.start) / f.duration).clamp(0.0, 1.0);
        let e = t * t * (3.0 - 2.0 * t);
        let pe = e * e * (3.0 - 2.0 * e); // the ground moves most while we are high up
        let dlon = camera::wrap_pi(f.to.lon - f.from.lon);
        let (la0, la1) = (f.from.alt.ln(), f.to.alt.ln());
        let extra = (f.peak.ln() - 0.5 * (la0 + la1)).max(0.0);
        let ln_alt = la0 + (la1 - la0) * e + extra * (std::f64::consts::PI * e).sin();
        self.cam = Camera {
            lon: f.from.lon + dlon * pe,
            lat: f.from.lat + (f.to.lat - f.from.lat) * pe,
            alt: ln_alt.exp(),
            pitch: f.from.pitch + (f.to.pitch - f.from.pitch) * e,
            heading: f.from.heading + camera::wrap_pi(f.to.heading - f.from.heading) * e,
            shift: [f.from.shift[0] + (f.to.shift[0] - f.from.shift[0]) * e, f.from.shift[1] + (f.to.shift[1] - f.from.shift[1]) * e],
        };
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

    /// The compass rose (bottom right): points to north, click to put north back up.
    fn compass(&mut self, ctx: &egui::Context, now: f64) {
        let heading = self.cam.heading as f32;
        let tilt = self.cam.pitch.to_degrees();
        let mut clicked = false;
        egui::Area::new(egui::Id::new("compass")).anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-14.0, -14.0)).show(ctx, |ui| {
            let size = 84.0;
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click());
            let p = ui.painter_at(rect.expand(2.0));
            let c = rect.center();
            let r = size * 0.5 - 2.0;
            p.circle(c, r, egui::Color32::from_black_alpha(150), egui::Stroke::new(1.5_f32, egui::Color32::from_white_alpha(200)));
            // screen direction of a compass bearing
            let dir = |bearing: f32| egui::vec2((bearing - heading).sin(), -(bearing - heading).cos());
            for k in 0..16 {
                let b = k as f32 * std::f32::consts::TAU / 16.0;
                let len = if k % 4 == 0 { 0.16 } else { 0.09 };
                let d = dir(b);
                p.line_segment([c + d * r * (1.0 - len), c + d * r * 0.97], egui::Stroke::new(1.2_f32, egui::Color32::from_white_alpha(170)));
            }
            let north = dir(0.0);
            let side = egui::vec2(-north.y, north.x);
            let tip = c + north * r * 0.78;
            let tail = c - north * r * 0.62;
            p.add(egui::Shape::convex_polygon(
                vec![tip, c + side * r * 0.2, c - north * r * 0.05, c - side * r * 0.2],
                egui::Color32::from_rgb(230, 60, 50),
                egui::Stroke::NONE,
            ));
            p.add(egui::Shape::convex_polygon(
                vec![tail, c + side * r * 0.2, c, c - side * r * 0.2],
                egui::Color32::from_gray(225),
                egui::Stroke::NONE,
            ));
            p.text(c + north * r * 0.5 + side * 0.0, egui::Align2::CENTER_CENTER, "N", egui::FontId::proportional(12.0), egui::Color32::WHITE);
            if tilt > 1.0 {
                p.text(
                    egui::pos2(c.x, rect.max.y - 1.0),
                    egui::Align2::CENTER_BOTTOM,
                    format!("{tilt:.0}°"),
                    egui::FontId::monospace(10.0),
                    egui::Color32::from_white_alpha(200),
                );
            }
            clicked = resp.on_hover_text("Click: north up and straight down (R)").clicked();
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

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = ctx.input(|i| i.time);
        let dt = ctx.input(|i| i.stable_dt) as f64;
        if dt > 0.0 {
            self.fps = self.fps * 0.9 + (1.0 / dt) * 0.1;
        }
        self.transit.poll();
        self.search.poll();

        let speed = SPEEDS[self.speed].1;
        // Flights advance by the real time since the previous frame, however far apart frames are.
        let anim_dt = if self.last_anim > 0.0 { (now - self.last_anim).clamp(0.0, 0.5) } else { 0.0 };
        self.last_anim = now;
        // The fastest the animation may redraw (the "animation rate" setting); trains never need more than 30.
        let frame = std::time::Duration::from_millis(1000 / self.anim_fps.max(1) as u64);
        let train_frame = frame.max(std::time::Duration::from_millis(33));
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
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.selected_line = None;
            self.tab = None;
            self.search_open = false;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::R)) && !ctx.wants_keyboard_input() {
            self.reset_orientation(now);
        }

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
                styles: style::table(zf, ppp, self.palette),
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
            let systems = self.transit.visible();
            // With a transit map in view the base map steps back: dimmed, and its labels hidden.
            let focus = if self.transit_focus { transit::focus(&self.cam, zf, w / h, &systems) } else { 0.0 };
            if focus > 0.01 && self.map_dim > 0.0 {
                let a = (focus * self.map_dim * 255.0).round() as u8;
                painter.rect_filled(rect, 0.0, egui::Color32::from_rgba_unmultiplied(12, 14, 22, a));
            }
            draw_routes(&painter, rect, &pr, &self.planes, &rs, &mut extras);
            // A click may hit a metro symbol (fly to that city) or a subway line (highlight it).
            let click = if resp.clicked() { resp.interact_pointer_pos() } else { None };
            let markers = self.transit.markers();
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
                    pick: if marker_click.is_none() { click } else { None },
                    selected: self.selected_line.as_ref().map(|(a, i)| (a, *i)),
                };
                transit::draw(&painter, rect, &self.cam, zf, &systems, &opts)
            };
            if out.animating {
                ctx.request_repaint_after(train_frame);
            }
            let line_hit = out.hit;
            for l in out.labels {
                extras.push(Cand::from_station(l));
            }
            transit::draw_markers(&painter, rect, &self.cam, zf, &markers);
            if let Some(mi) = marker_click {
                let (lat, lon) = markers[mi].home;
                self.goto_city(lon, lat, now);
            } else if click.is_some() {
                if line_hit.is_none() {
                    self.tab = None;
                    self.search_open = false;
                }
                self.selected_line = line_hit;
            }
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
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait.max(frame.as_secs_f64())));
            }

            // --- the aircraft themselves: above the routes and station markers, below the labels ---
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

            // --- labels ---
            if self.show_labels {
                let ls = LabelStyle { scale: LABEL_SIZES[self.label_size].1, bold: self.label_bold, dark: self.palette.dark(), family: fonts::Fonts::family(), real_bold: self.fonts.has_real_bold() };
                let map_labels = if focus > 0.5 { &frame_tiles.labels[..0] } else { &frame_tiles.labels[..] };
                draw_labels(&painter, rect, &pr, zf, map_labels, extras, &ls);
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

        if self.show_compass {
            self.compass(ctx, now);
        }
        self.controls(ctx, rect_for_ui, now, zf_for_ui);

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
}

impl Cand {
    fn new(priority: f32, text: String, size: f32, color: [u8; 3], halo: [u8; 3], at: egui::Pos2) -> Self {
        Cand { priority, text, size, color, halo, at }
    }

    fn from_station(l: StationLabel) -> Self {
        let priority = if l.interchange { 9.5 } else { 17.0 };
        Cand::new(priority, l.text, if l.interchange { 12.5 } else { 11.5 }, [20, 20, 30], [255, 255, 255], l.at)
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
) {
    let (w, h) = (rect.width() as f64, rect.height() as f64);
    for t in tiles {
        for l in t.iter() {
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
            cands.push(Cand::new(rule.priority, text, rule.size, rule.color, rule.halo, rect.min + egui::vec2(s[0] as f32, s[1] as f32)));
        }
    }
    cands.sort_by(|a, b| a.priority.partial_cmp(&b.priority).unwrap_or(std::cmp::Ordering::Equal));

    let mut placed: Vec<egui::Rect> = Vec::new();
    let fake_bold = ls.bold && !ls.real_bold;
    let bold = if fake_bold { 0.45_f32 } else { 0.0 };
    let family = if ls.bold && ls.real_bold { fonts::Fonts::bold_family() } else { ls.family.clone() };
    for c in cands.into_iter().take(700) {
        if placed.len() >= 220 {
            break;
        }
        let font = egui::FontId::new(c.size * ls.scale, family.clone());
        let col = egui::Color32::from_rgb(c.color[0], c.color[1], c.color[2]);
        let halo = egui::Color32::from_rgb(c.halo[0], c.halo[1], c.halo[2]);
        let main = painter.layout_no_wrap(c.text.clone(), font.clone(), col);
        let r = egui::Rect::from_center_size(c.at, main.size());
        let padded = r.expand2(egui::vec2(4.0, 2.0));
        if placed.iter().any(|p| p.intersects(padded)) {
            continue;
        }
        placed.push(padded);

        let shadow = painter.layout_no_wrap(c.text, font, halo);
        let o = 1.3 + bold * 1.4;
        for (dx, dy) in [(-o, 0.0), (o, 0.0), (0.0, -o), (0.0, o), (-o, -o), (o, -o), (-o, o), (o, o)] {
            painter.galley(r.min + egui::vec2(dx, dy), shadow.clone(), halo);
        }
        if fake_bold {
            // No bold face is installed: fatten the strokes by drawing the text a little off-centre.
            for (dx, dy) in [(-bold, 0.0), (bold, 0.0), (0.0, -bold * 0.6), (0.0, bold * 0.6)] {
                painter.galley(r.min + egui::vec2(dx, dy), main.clone(), col);
            }
        }
        painter.galley(r.min, main, col);
    }
}
