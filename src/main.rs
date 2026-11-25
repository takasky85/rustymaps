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
mod style;
mod tile_id;
mod tiles;
mod transit;
mod transit_data;

use std::sync::Arc;

use eframe::{egui, egui_wgpu, wgpu};

use camera::{dot, merc_to_vec, Camera, Projector, MAX_ALT, MAX_PITCH, MIN_ALT, V3};
use flight::{AircraftDef, Route, Sim, AIRCRAFT};
use gpu::{Globals, MapCallback};
use plane::{PlaneCallback, PlaneDraw};
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
}

/// How route lines are drawn (adjustable in the Flights section).
struct RouteStyle {
    color: [u8; 3],
    alpha: f32,
    width: f32,
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

        App {
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
        }
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
        let alt = Camera::alt_for_zoom(CITY_ZOOM, lat.to_radians(), self.view_h);
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
        let mut to = Camera { lon, lat, alt: (8.0 * len).max(3.0 * height), pitch: 0.9, heading: self.cam.heading };
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
            .default_width(290.0)
            .resizable(false)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().max_height((rect.height() - 90.0).max(200.0)).auto_shrink([true, true]).show(ui, |ui| {
                    ui.label(t(format!(
                        "{:.0} fps   zoom {:.2}   tilt {:.0}°   rot {:.0}°",
                        self.fps,
                        zf,
                        self.cam.pitch.to_degrees(),
                        self.cam.heading.to_degrees()
                    )));
                    ui.label(t(format!("{:.4}°, {:.4}°", self.cam.lat.to_degrees(), self.cam.lon.to_degrees())));
                    ui.label(t(format!(
                        "tiles: {} drawn, {} cached, {} loading{}",
                        self.stats.drawn,
                        self.stats.loaded,
                        self.stats.pending,
                        if self.stats.failed > 0 { format!(", {} retrying", self.stats.failed) } else { String::new() }
                    )));
                    ui.label(t(format!("GPU: {}", self.gpu_name)));
                    ui.small("left-drag: move   right- or middle-drag: tilt (up/down) + rotate (left/right)   wheel: zoom   double-click: zoom in   R: reset view");

                    egui::CollapsingHeader::new("View").default_open(true).show(ui, |ui| {
                        let mut tilt = self.cam.pitch.to_degrees();
                        if ui.add(egui::Slider::new(&mut tilt, 0.0..=MAX_PITCH.to_degrees()).text("Tilt °")).changed() {
                            self.cam.pitch = tilt.to_radians();
                            self.fly = None;
                        }
                        let mut rot = self.cam.heading.to_degrees();
                        if ui.add(egui::Slider::new(&mut rot, -180.0..=180.0).text("Rotate °")).changed() {
                            self.cam.heading = rot.to_radians();
                            self.fly = None;
                        }
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("Reset view").on_hover_text("north up, looking straight down (R)").clicked() {
                                self.reset_orientation(now);
                            }
                            if ui.button("Globe").clicked() {
                                let mut to = self.cam;
                                to.alt = 2.2;
                                to.pitch = 0.0;
                                to.heading = 0.0;
                                self.fly_to(to, now, 2.5);
                            }
                            if ui.button("Clear cache").clicked() {
                                self.tiles.clear_cache();
                            }
                        });
                        ui.checkbox(&mut self.show_compass, "Compass");
                        ui.horizontal(|ui| {
                            ui.label("Go to");
                            egui::ComboBox::from_id_salt("goto").selected_text("choose a city…").show_ui(ui, |ui| {
                                if ui.selectable_label(false, "San Francisco").clicked() {
                                    self.goto_city(SF_LON, SF_LAT, now);
                                }
                                for sys in SYSTEMS.iter() {
                                    if ui.selectable_label(false, sys.title).clicked() {
                                        self.goto_city(sys.home.1, sys.home.0, now);
                                    }
                                }
                            });
                        });
                    });

                    egui::CollapsingHeader::new("Map style & labels").default_open(true).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Style");
                            egui::ComboBox::from_id_salt("palette").selected_text(self.palette.name()).width(190.0).show_ui(ui, |ui| {
                                for p in Palette::all() {
                                    ui.selectable_value(&mut self.palette, p, p.name());
                                }
                            });
                        });
                        ui.checkbox(&mut self.show_labels, "Show labels");
                        ui.horizontal(|ui| {
                            for (i, (name, _)) in LABEL_SIZES.iter().enumerate() {
                                ui.radio_value(&mut self.label_size, i, *name);
                            }
                        });
                        ui.checkbox(&mut self.label_bold, "Bold labels");
                        ui.horizontal(|ui| {
                            ui.label("Label font");
                            let names = self.fonts.names();
                            let before = self.fonts.selected;
                            egui::ComboBox::from_id_salt("label_font")
                                .selected_text(names[self.fonts.selected.min(names.len() - 1)])
                                .width(150.0)
                                .show_ui(ui, |ui| {
                                    for (i, n) in names.iter().enumerate() {
                                        ui.selectable_value(&mut self.fonts.selected, i, *n);
                                    }
                                });
                            if self.fonts.selected != before {
                                self.fonts.apply(ctx);
                            }
                        });
                        ui.small("Styles are colour recreations of well-known OpenStreetMap looks on the same vector data.");
                    });

                    egui::CollapsingHeader::new(format!("Transit maps ({})", SYSTEMS.len())).default_open(false).show(ui, |ui| {
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
                                    _ => {
                                        if !self.transit.offline(i) && !on {
                                            ui.weak("download");
                                        }
                                    }
                                }
                            });
                            if let Status::Failed(e) = self.transit.status(i) {
                                ui.small(egui::RichText::new(e).color(egui::Color32::LIGHT_RED));
                            }
                        }
                        ui.small(format!(
                            "Each city with a saved map shows a metro symbol; the lines fade in from zoom {:.0}, and station circles and names appear once there is room for them. Saved maps are read from {}",
                            transit::MAP_FROM,
                            self.transit.dir().join("transit").display()
                        ));
                    });

                    egui::CollapsingHeader::new("Flights").default_open(true).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt("speed").selected_text(SPEEDS[self.speed].0).show_ui(ui, |ui| {
                                for (i, (name, _)) in SPEEDS.iter().enumerate() {
                                    ui.selectable_value(&mut self.speed, i, *name);
                                }
                            });
                            if ui.button("▶ all").clicked() {
                                for p in &mut self.planes {
                                    p.sim.playing = true;
                                }
                            }
                            if ui.button("⏸ all").clicked() {
                                for p in &mut self.planes {
                                    p.sim.playing = false;
                                }
                            }
                        });
                        ui.add(egui::Slider::new(&mut self.plane_size, 1.0..=20000.0).logarithmic(true).text("Size ×"));
                        ui.add(egui::Slider::new(&mut self.exaggeration, 1.0..=150.0).logarithmic(true).text("Altitude ×"));
                        ui.horizontal(|ui| {
                            ui.label("Route lines");
                            ui.color_edit_button_srgb(&mut self.route_color);
                        });
                        ui.add(egui::Slider::new(&mut self.route_alpha, 0.1..=1.0).text("Opacity"));
                        ui.add(egui::Slider::new(&mut self.route_width, 1.0..=10.0).text("Width"));
                        if !self.model_errors.is_empty() {
                            ui.colored_label(egui::Color32::LIGHT_RED, "Some aircraft models were not found:");
                            for e in &self.model_errors {
                                ui.small(egui::RichText::new(e).color(egui::Color32::LIGHT_RED));
                            }
                            ui.small(format!("Put the data folder next to the program or at {}", self.data_dir.display()));
                        }
                        let mut look: Option<usize> = None;
                        for i in 0..self.planes.len() {
                            ui.separator();
                            let p = &mut self.planes[i];
                            let st = p.sim.state(&p.route);
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut p.show, format!("{}  {} → {}", p.def.label, p.def.from.0, p.def.to.0));
                            });
                            ui.horizontal(|ui| {
                                if ui.button(if p.sim.playing { "⏸" } else { "▶" }).clicked() {
                                    p.sim.playing = !p.sim.playing;
                                }
                                if ui.button("⏮").on_hover_text("back to the start").clicked() {
                                    p.sim.set_t(0.0);
                                    p.sim.outbound = true;
                                }
                                if ui.button("Look at").clicked() {
                                    look = Some(i);
                                }
                                let mut f = self.follow == Some(i);
                                if ui.checkbox(&mut f, "Follow").changed() {
                                    self.follow = if f { Some(i) } else { None };
                                }
                            });
                            ui.spacing_mut().slider_width = 230.0;
                            let mut tt = p.sim.t;
                            if ui.add(egui::Slider::new(&mut tt, 0.0..=1.0).show_value(false)).changed() {
                                p.sim.set_t(tt);
                            }
                            let status = if p.sim.pause > 0.0 {
                                "turning around"
                            } else if p.sim.outbound {
                                "outbound"
                            } else {
                                "returning"
                            };
                            ui.label(t(format!(
                                "{:.0}%  {:.1} h  {:.0} ft  {}",
                                p.sim.t * 100.0,
                                p.sim.t * p.route.duration_s() / 3600.0,
                                st.alt_m / 0.3048,
                                status
                            )));
                        }
                        if let Some(i) = look {
                            self.planes[i].show = true;
                            self.look_at_plane(i, now);
                        }
                        ui.small("Aircraft shuttle between their airports until paused. Altitude × lifts them above the ground so they read as flying.");
                    });
                });
            });
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

fn vec_lon_lat(p: V3) -> (f64, f64) {
    camera::vec_to_lonlat(p)
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = ctx.input(|i| i.time);
        let dt = ctx.input(|i| i.stable_dt) as f64;
        if dt > 0.0 {
            self.fps = self.fps * 0.9 + (1.0 / dt) * 0.1;
        }
        self.transit.poll();

        let speed = SPEEDS[self.speed].1;
        let mut animating = false;
        for p in &mut self.planes {
            if p.sim.playing {
                p.sim.advance(dt.min(0.1), speed, p.route.duration_s());
                animating = true;
            }
        }
        if animating {
            ctx.request_repaint();
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
                    self.follow = None;
                    self.cam.pan(d.x as f64, d.y as f64, h);
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
                },
            ));

            // --- overlays (egui), bottom to top: routes, transit, plane drop lines ---
            let pr = self.cam.projector(w, h);
            let mut extras: Vec<Cand> = Vec::new();
            let rs = RouteStyle { color: self.route_color, alpha: self.route_alpha, width: self.route_width };
            draw_routes(&painter, rect, &pr, &self.planes, &rs, &mut extras);
            let systems = self.transit.visible();
            for l in transit::draw(&painter, rect, &self.cam, zf, &systems) {
                extras.push(Cand::from_station(l));
            }
            transit::draw_markers(&painter, rect, &self.cam, zf, &self.transit.markers());
            let mut draws: Vec<PlaneDraw> = Vec::new();
            for (i, p) in self.planes.iter().enumerate() {
                let (true, Some(radius)) = (p.show, p.radius_m) else { continue };
                let st = p.sim.state(&p.route);
                draw_plane_marker(&painter, rect, &self.cam, &pr, &st, p.def, self.plane_size, self.exaggeration);
                if let Some((mvp, nrm)) = flight::plane_matrices(&self.cam, w / h, &st, self.plane_size, self.exaggeration, radius) {
                    draws.push(PlaneDraw { model: i, mvp, nrm });
                }
            }

            // --- the aircraft themselves: above the routes and station markers, below the labels ---
            if !draws.is_empty() {
                painter.add(egui_wgpu::Callback::new_paint_callback(
                    rect,
                    PlaneCallback {
                        draws,
                        srgb: self.srgb,
                        viewport_px: [(w as f32 * ppp) as u32, (h as f32 * ppp) as u32],
                    },
                ));
            }

            // --- labels ---
            if self.show_labels {
                let ls = LabelStyle { scale: LABEL_SIZES[self.label_size].1, bold: self.label_bold, dark: self.palette.dark(), family: fonts::Fonts::family() };
                draw_labels(&painter, rect, &pr, zf, &frame_tiles.labels, extras, &ls);
            }

            // --- attribution (required by the data and model licences) ---
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
                            ui.label("· Models (CC-BY-4.0): 737 by");
                            ui.hyperlink_to(
                                "A Random Modeler",
                                "https://sketchfab.com/3d-models/southwest-airlines-boeing-737-73d0cd0c03594c5d9c53235d74b429bf",
                            );
                            ui.label("A380 by");
                            ui.hyperlink_to(
                                "Nobilis 2",
                                "https://sketchfab.com/3d-models/airbus-a380-841-lufthansa-edbffac71f88480c82f5690b777e7416",
                            );
                            ui.label("787 by");
                            ui.hyperlink_to(
                                "themaker845",
                                "https://sketchfab.com/3d-models/boeing-787-dreamliner-524f485c748146b7968e0dd8eb84e786",
                            );
                        });
                    });
            });
        });

        if self.show_compass {
            self.compass(ctx, now);
        }
        self.controls(ctx, rect_for_ui, now, zf_for_ui);
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
        for v in p.route.polyline(360) {
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
    let bold = if ls.bold { 0.45_f32 } else { 0.0 };
    for c in cands.into_iter().take(700) {
        if placed.len() >= 220 {
            break;
        }
        let font = egui::FontId::new(c.size * ls.scale, ls.family.clone());
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
        if ls.bold {
            // No bold font is bundled: fatten the strokes by drawing the text a little off-centre.
            for (dx, dy) in [(-bold, 0.0), (bold, 0.0), (0.0, -bold * 0.6), (0.0, bold * 0.6)] {
                painter.galley(r.min + egui::vec2(dx, dy), main.clone(), col);
            }
        }
        painter.galley(r.min, main, col);
    }
}
