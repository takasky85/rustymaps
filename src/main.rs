//! RustyMaps: a small globe-to-street-level vector map viewer.
//!
//! Rust + egui + wgpu. Tiles come from OpenFreeMap (vector, OpenMapTiles schema),
//! are drawn on the GPU by a custom paint callback, and labels are drawn by egui.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod camera;
mod flight;
mod gpu;
mod json;
mod mesh;
mod mvt;
mod plane;
mod style;
mod tile_id;
mod tiles;
mod transit;
mod transit_data;

use std::sync::Arc;

use eframe::{egui, egui_wgpu, wgpu};

use camera::{dot, merc_to_vec, Camera, MAX_ALT, MAX_PITCH, MIN_ALT, V3};
use gpu::{Globals, MapCallback};
use plane::PlaneFrame;
use style::Palette;
use tiles::TileManager;
use transit::{StationLabel, Status, TransitManager};
use transit_data::SYSTEMS;

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


/// A smooth camera move between two views.
struct Fly {
    from: Camera,
    to: Camera,
    start: f64,
    duration: f64,
}

/// Playback speeds for the simulated flight (multiples of real time).
const SPEEDS: [(&str, f64); 4] = [("real time", 1.0), ("60x", 60.0), ("600x", 600.0), ("3600x", 3600.0)];

struct Stats {
    drawn: usize,
    loaded: usize,
    pending: usize,
    failed: usize,
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
    // flight
    route: flight::Route,
    show_flight: bool,
    flight_t: f64,
    playing: bool,
    speed: usize,
    plane_size: f64,
    follow: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let rs = cc.wgpu_render_state.clone().expect("the wgpu renderer is required");
        let info = rs.adapter.get_info();
        let gpu_name = format!("{} ({:?})", info.name, info.backend);
        let srgb = rs.target_format.is_srgb();
        let tiles = TileManager::new(&cc.egui_ctx, rs);
        let transit_dir = tiles.cache_dir().parent().map(|p| p.join("transit")).unwrap_or_else(|| "transit".into());
        App {
            cam: Camera::from_degrees(-115.0, 32.0, 2.2),
            transit: TransitManager::new(&cc.egui_ctx, transit_dir),
            tiles,
            palette: Palette::Standard,
            fly: None,
            srgb,
            gpu_name,
            fps: 60.0,
            stats: Stats { drawn: 0, loaded: 0, pending: 0, failed: 0 },
            view_h: 800.0,
            route: flight::Route::sfo_to_hnd(),
            show_flight: true,
            flight_t: 0.35,
            playing: false,
            speed: 2,
            plane_size: 300.0,
            follow: false,
        }
    }

    fn fly_to(&mut self, to: Camera, now: f64, duration: f64) {
        self.follow = false;
        self.fly = Some(Fly { from: self.cam, to, start: now, duration });
    }

    fn goto_city(&mut self, lon: f64, lat: f64, now: f64) {
        let alt = Camera::alt_for_zoom(CITY_ZOOM, lat.to_radians(), self.view_h);
        let mut to = Camera::from_degrees(lon, lat, alt);
        to.pitch = self.cam.pitch;
        self.fly_to(to, now, 4.5);
    }

    fn look_at_plane(&mut self, now: f64) {
        let st = self.route.state(self.flight_t);
        let (lon, lat) = self.route.lonlat_deg(self.flight_t);
        let len = flight::PLANE_LENGTH_M * self.plane_size / flight::EARTH_RADIUS_M;
        let height = st.alt_m / flight::EARTH_RADIUS_M;
        let mut to = Camera::from_degrees(lon, lat, (8.0 * len).max(3.0 * height));
        to.pitch = 0.9;
        self.fly_to(to, now, 1.6);
    }

    fn step_fly(&mut self, now: f64) -> bool {
        let Some(f) = &self.fly else { return false };
        let t = ((now - f.start) / f.duration).clamp(0.0, 1.0);
        let e = t * t * (3.0 - 2.0 * t);
        let dlon = camera::wrap_pi(f.to.lon - f.from.lon);
        self.cam = Camera {
            lon: f.from.lon + dlon * e,
            lat: f.from.lat + (f.to.lat - f.from.lat) * e,
            alt: (f.from.alt.ln() + (f.to.alt.ln() - f.from.alt.ln()) * e).exp(),
            pitch: f.from.pitch + (f.to.pitch - f.from.pitch) * e,
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

    fn controls(&mut self, ctx: &egui::Context, rect: egui::Rect, now: f64, zf: f64) {
        let t = |s: String| egui::RichText::new(s).monospace().size(12.0);
        egui::Window::new("RustyMaps")
            .default_pos(rect.min + egui::vec2(10.0, 10.0))
            .default_width(270.0)
            .resizable(false)
            .show(ctx, |ui| {
                let lat = self.cam.lat.to_degrees();
                let lon = self.cam.lon.to_degrees();
                ui.label(t(format!("{:.0} fps   zoom {:.2}   tilt {:.0}°", self.fps, zf, self.cam.pitch.to_degrees())));
                ui.label(t(format!("{lat:.4}°, {lon:.4}°")));
                ui.label(t(format!(
                    "tiles: {} drawn, {} cached, {} loading{}",
                    self.stats.drawn,
                    self.stats.loaded,
                    self.stats.pending,
                    if self.stats.failed > 0 { format!(", {} retrying", self.stats.failed) } else { String::new() }
                )));
                ui.label(t(format!("GPU: {}", self.gpu_name)));
                ui.small("drag: pan   right-drag: tilt   wheel: zoom   double-click: zoom in");

                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Style");
                    egui::ComboBox::from_id_salt("palette").selected_text(self.palette.name()).show_ui(ui, |ui| {
                        for p in Palette::ALL {
                            ui.selectable_value(&mut self.palette, p, p.name());
                        }
                    });
                });
                let mut tilt = self.cam.pitch.to_degrees();
                if ui.add(egui::Slider::new(&mut tilt, 0.0..=MAX_PITCH.to_degrees()).text("Tilt °")).changed() {
                    self.cam.pitch = tilt.to_radians();
                    self.fly = None;
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Globe").clicked() {
                        let mut to = self.cam;
                        to.alt = 2.2;
                        to.pitch = 0.0;
                        self.fly_to(to, now, 2.5);
                    }
                    if ui.button("San Francisco").clicked() {
                        self.goto_city(SF_LON, SF_LAT, now);
                    }
                    for sys in SYSTEMS.iter().skip(1) {
                        let name = sys.title.split_whitespace().next().unwrap_or("");
                        let name = if sys.id == "lametro" { "Los Angeles" } else { name };
                        if ui.button(name).clicked() {
                            self.goto_city(sys.home.1, sys.home.0, now);
                        }
                    }
                    if ui.button("Clear cache").clicked() {
                        self.tiles.clear_cache();
                    }
                });

                ui.separator();
                ui.strong("Transit maps");
                for (i, sys) in SYSTEMS.iter().enumerate() {
                    let mut on = self.transit.enabled(i);
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut on, sys.title).changed() {
                            self.transit.set_enabled(i, on);
                        }
                        match self.transit.status(i) {
                            Status::Loading => {
                                ui.spinner();
                            }
                            Status::Failed(_) => {
                                ui.colored_label(egui::Color32::LIGHT_RED, "failed");
                            }
                            _ => {}
                        }
                    });
                    if let Status::Failed(e) = self.transit.status(i) {
                        ui.small(egui::RichText::new(e).color(egui::Color32::LIGHT_RED));
                    }
                }
                ui.small("Shown from zoom 8; stations from 10.5. Data: OpenStreetMap.");

                ui.separator();
                ui.horizontal(|ui| {
                    ui.strong("Flight: San Francisco → Tokyo");
                });
                ui.checkbox(&mut self.show_flight, "Show route and plane");
                let st = self.route.state(self.flight_t);
                ui.horizontal(|ui| {
                    let label = if self.playing { "⏸ Pause" } else { "▶ Fly" };
                    if ui.button(label).clicked() {
                        if self.flight_t >= 1.0 {
                            self.flight_t = 0.0;
                        }
                        self.playing = !self.playing;
                    }
                    if ui.button("⏮").on_hover_text("back to San Francisco").clicked() {
                        self.flight_t = 0.0;
                    }
                    egui::ComboBox::from_id_salt("speed").selected_text(SPEEDS[self.speed].0).show_ui(ui, |ui| {
                        for (i, (name, _)) in SPEEDS.iter().enumerate() {
                            ui.selectable_value(&mut self.speed, i, *name);
                        }
                    });
                });
                ui.spacing_mut().slider_width = 220.0;
                ui.add(egui::Slider::new(&mut self.flight_t, 0.0..=1.0).show_value(false));
                let elapsed_h = self.flight_t * self.route.duration_s() / 3600.0;
                let (flon, flat) = self.route.lonlat_deg(self.flight_t);
                ui.label(t(format!(
                    "{:.0}%  {:.1} h   {:.0} ft",
                    self.flight_t * 100.0,
                    elapsed_h,
                    st.alt_m / 0.3048
                )));
                ui.label(t(format!("{flat:.2}°, {flon:.2}°")));
                ui.add(
                    egui::Slider::new(&mut self.plane_size, 1.0..=20000.0)
                        .logarithmic(true)
                        .text("Plane size ×"),
                );
                ui.horizontal(|ui| {
                    if ui.button("Look at plane").clicked() {
                        self.show_flight = true;
                        self.look_at_plane(now);
                    }
                    ui.checkbox(&mut self.follow, "Follow");
                });
                ui.small("The plane flies at its real altitude, so it is only in front of the camera once you are higher than ~11 km (zoom out) — or use Look at plane.");
            });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = ctx.input(|i| i.time);
        let dt = ctx.input(|i| i.stable_dt) as f64;
        if dt > 0.0 {
            self.fps = self.fps * 0.9 + (1.0 / dt) * 0.1;
        }
        self.transit.poll();

        if self.playing {
            self.flight_t += dt.min(0.1) * SPEEDS[self.speed].1 / self.route.duration_s();
            if self.flight_t >= 1.0 {
                self.flight_t = 1.0;
                self.playing = false;
            }
            ctx.request_repaint();
        }
        if self.step_fly(now) {
            ctx.request_repaint();
        } else if self.follow && self.show_flight {
            let (lon, lat) = self.route.lonlat_deg(self.flight_t);
            self.cam.lon = lon.to_radians();
            self.cam.lat = lat.to_radians();
            self.cam.normalize();
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
            if resp.dragged_by(egui::PointerButton::Primary) {
                let d = resp.drag_delta();
                if d != egui::Vec2::ZERO {
                    self.fly = None;
                    self.follow = false;
                    self.cam.pan(d.x as f64, d.y as f64, h);
                }
            }
            if resp.dragged_by(egui::PointerButton::Secondary) {
                let d = resp.drag_delta();
                self.fly = None;
                self.cam.pitch = (self.cam.pitch - d.y as f64 * 0.006).clamp(0.0, MAX_PITCH);
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
                        let mut to = Camera { lon, lat, alt: self.cam.alt * 0.35, pitch: self.cam.pitch };
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

            // --- 3D plane ---
            let flight_state = self.route.state(self.flight_t);
            let plane = if self.show_flight {
                flight::plane_matrices(&self.cam, w / h, &flight_state, self.plane_size)
                    .map(|(mvp, nrm)| PlaneFrame { mvp, nrm, srgb: self.srgb })
            } else {
                None
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
                    plane,
                    viewport_px: [(w as f32 * ppp) as u32, (h as f32 * ppp) as u32],
                    draw: frame_tiles.draw.clone(),
                },
            ));

            // --- overlays (egui) ---
            let mut extras: Vec<Cand> = Vec::new();
            if self.show_flight {
                draw_route(&painter, rect, &self.cam, &self.route, &flight_state, self.plane_size, &mut extras);
            }
            let systems = self.transit.visible();
            for l in transit::draw(&painter, rect, &self.cam, zf, &systems) {
                extras.push(Cand::from_station(l));
            }
            draw_labels(&painter, rect, &self.cam, zf, &frame_tiles.labels, extras);

            // --- attribution (required by the data licences) ---
            egui::Area::new(egui::Id::new("attribution"))
                .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-8.0, -8.0))
                .show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(egui::Color32::from_black_alpha(170))
                        .corner_radius(4.0)
                        .inner_margin(egui::Margin::symmetric(6, 3))
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                ui.hyperlink_to("OpenFreeMap", "https://openfreemap.org");
                                ui.hyperlink_to("© OpenMapTiles", "https://openmaptiles.org/");
                                ui.label("Data from");
                                ui.hyperlink_to("OpenStreetMap", "https://www.openstreetmap.org/copyright");
                                ui.label("· 737 model:");
                                ui.hyperlink_to(
                                    "A Random Modeler (CC-BY-4.0)",
                                    "https://sketchfab.com/3d-models/southwest-airlines-boeing-737-73d0cd0c03594c5d9c53235d74b429bf",
                                );
                            });
                        });
                });
        });

        self.controls(ctx, rect_for_ui, now, zf_for_ui);
    }
}

// ---------------------------------------------------------------------------
// Overlays
// ---------------------------------------------------------------------------

fn visible_on_globe(cam: &Camera, p: V3) -> bool {
    dot(p, cam.pos()) > 1.0 + 1e-9
}

/// The route line, airport markers and a ring that keeps a tiny plane findable.
fn draw_route(
    painter: &egui::Painter,
    rect: egui::Rect,
    cam: &Camera,
    route: &flight::Route,
    st: &flight::FlightState,
    size_mul: f64,
    labels: &mut Vec<Cand>,
) {
    let (w, h) = (rect.width() as f64, rect.height() as f64);
    let to_screen = |p: V3| -> Option<egui::Pos2> {
        if !visible_on_globe(cam, p) {
            return None;
        }
        cam.project(p, w, h).map(|s| rect.min + egui::vec2(s[0] as f32, s[1] as f32))
    };

    let mut run: Vec<egui::Pos2> = Vec::new();
    let mut runs: Vec<Vec<egui::Pos2>> = Vec::new();
    for p in route.polyline(360) {
        match to_screen(p) {
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
    for r in &runs {
        painter.add(egui::Shape::line(r.clone(), egui::Stroke::new(5.0, egui::Color32::from_black_alpha(110))));
    }
    for r in &runs {
        painter.add(egui::Shape::line(r.clone(), egui::Stroke::new(2.5, egui::Color32::from_rgb(255, 140, 0))));
    }

    for (name, (lon, lat)) in [("SFO", flight::SFO), ("HND", flight::HND)] {
        let p = lonlat_vec(lon, lat);
        if let Some(s) = to_screen(p) {
            painter.circle(s, 4.0, egui::Color32::WHITE, egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 140, 0)));
            labels.push(Cand::new(0.5, name.to_string(), 12.0, [255, 120, 0], [255, 255, 255], s + egui::vec2(0.0, -14.0)));
        }
    }

    // Ring around the plane while it is smaller than ~14 px, so it can be found from afar.
    let (pos, _r, fwd, _u) = flight::plane_frame(st);
    let nose = camera_add(pos, fwd, 0.5 * flight::PLANE_LENGTH_M * size_mul / flight::EARTH_RADIUS_M);
    if let (Some(a), Some(b)) = (cam.project(pos, w, h), cam.project(nose, w, h)) {
        if (a[0] - b[0]).hypot(a[1] - b[1]) < 7.0 && !occluded(cam, pos) {
            let c = rect.min + egui::vec2(a[0] as f32, a[1] as f32);
            if rect.contains(c) {
                painter.circle_stroke(c, 11.0, egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 140, 0)));
            }
        }
    }
}

fn lonlat_vec(lon_deg: f64, lat_deg: f64) -> V3 {
    camera::lonlat_to_vec(lon_deg.to_radians(), lat_deg.to_radians())
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
        Cand::new(priority, l.text, if l.interchange { 12.0 } else { 11.0 }, [20, 20, 30], [255, 255, 255], l.at)
    }
}

/// Places labels greedily by priority; a label that would overlap one already placed is dropped.
fn draw_labels(
    painter: &egui::Painter,
    rect: egui::Rect,
    cam: &Camera,
    zf: f64,
    tiles: &[Arc<Vec<mesh::Label>>],
    mut cands: Vec<Cand>,
) {
    let (w, h) = (rect.width() as f64, rect.height() as f64);
    for t in tiles {
        for l in t.iter() {
            let Some(rule) = style::label_rule(l.kind, l.rank, zf) else { continue };
            let p = merc_to_vec(l.mx, l.my);
            if !visible_on_globe(cam, p) {
                continue;
            }
            let Some(s) = cam.project(p, w, h) else { continue };
            if s[0] < -40.0 || s[0] > w + 40.0 || s[1] < -20.0 || s[1] > h + 20.0 {
                continue;
            }
            let text = if rule.upper { l.text.to_uppercase() } else { l.text.clone() };
            cands.push(Cand::new(
                rule.priority,
                text,
                rule.size,
                rule.color,
                rule.halo,
                rect.min + egui::vec2(s[0] as f32, s[1] as f32),
            ));
        }
    }
    cands.sort_by(|a, b| a.priority.partial_cmp(&b.priority).unwrap_or(std::cmp::Ordering::Equal));

    let mut placed: Vec<egui::Rect> = Vec::new();
    for c in cands.into_iter().take(600) {
        if placed.len() >= 200 {
            break;
        }
        let font = egui::FontId::proportional(c.size);
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
        let o = 1.3;
        for (dx, dy) in [(-o, 0.0), (o, 0.0), (0.0, -o), (0.0, o), (-o, -o), (o, -o), (-o, o), (o, o)] {
            painter.galley(r.min + egui::vec2(dx, dy), shadow.clone(), halo);
        }
        painter.galley(r.min, main, col);
    }
}
