//! RustyMaps: a small globe-to-street-level vector map viewer.
//!
//! Rust + egui + wgpu. Tiles come from OpenFreeMap (vector, OpenMapTiles schema),
//! are drawn on the GPU by a custom paint callback, and labels are drawn by egui.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod camera;
mod gpu;
mod mesh;
mod mvt;
mod style;
mod tile_id;
mod tiles;

use std::sync::Arc;

use eframe::{egui, egui_wgpu, wgpu};

use camera::{dot, merc_to_vec, Camera, MAX_ALT, MIN_ALT};
use gpu::{Globals, MapCallback};
use tiles::TileManager;

const SF_LON: f64 = -122.4194;
const SF_LAT: f64 = 37.7749;
const SF_ZOOM: f64 = 12.5;

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

struct App {
    cam: Camera,
    tiles: TileManager,
    fly: Option<Fly>,
    srgb: bool,
    gpu_name: String,
    fps: f64,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let rs = cc.wgpu_render_state.clone().expect("the wgpu renderer is required");
        let info = rs.adapter.get_info();
        let gpu_name = format!("{} ({:?})", info.name, info.backend);
        let srgb = rs.target_format.is_srgb();
        App {
            cam: Camera::from_degrees(-115.0, 32.0, 2.2),
            tiles: TileManager::new(&cc.egui_ctx, rs),
            fly: None,
            srgb,
            gpu_name,
            fps: 60.0,
        }
    }

    fn fly_to(&mut self, to: Camera, now: f64, duration: f64) {
        self.fly = Some(Fly { from: self.cam, to, start: now, duration });
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
        };
        self.cam.normalize();
        if t >= 1.0 {
            self.fly = None;
        }
        true
    }

    /// Multiplies altitude by `factor`, keeping the point under `cursor` fixed.
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
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = ctx.input(|i| i.time);
        let dt = ctx.input(|i| i.stable_dt) as f64;
        if dt > 0.0 {
            self.fps = self.fps * 0.9 + (1.0 / dt) * 0.1;
        }
        if self.step_fly(now) {
            ctx.request_repaint();
        }

        let mut clear_cache = false;
        let mut go_globe = false;
        let mut go_sf = false;

        egui::CentralPanel::default().frame(egui::Frame::new()).show(ctx, |ui| {
            let rect = ui.max_rect();
            let resp = ui.allocate_rect(rect, egui::Sense::click_and_drag());
            let (w, h) = (rect.width() as f64, rect.height() as f64);
            if w < 8.0 || h < 8.0 {
                return;
            }

            // --- input ---
            if resp.dragged_by(egui::PointerButton::Primary) {
                let d = resp.drag_delta();
                if d != egui::Vec2::ZERO {
                    self.fly = None;
                    self.cam.pan(d.x as f64, d.y as f64, h);
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
                        let mut to = Camera { lon, lat, alt: self.cam.alt * 0.35 };
                        to.normalize();
                        self.fly_to(to, now, 0.9);
                    }
                }
            }

            // --- tiles ---
            let zf = self.cam.zoom_f(h);
            let frame_tiles = self.tiles.update(&self.cam, w, h);

            // --- map (GPU) ---
            let ppp = ctx.pixels_per_point();
            let pos = self.cam.pos();
            let globals = Globals {
                view_proj: self.cam.view_proj_cols(w / h),
                proj: self.cam.proj_cols(w / h),
                cam_pos: [pos[0] as f32, pos[1] as f32, pos[2] as f32, if self.srgb { 1.0 } else { 0.0 }],
                cam_rel: [self.cam.lat.cos() as f32, self.cam.lat.sin() as f32, self.cam.alt as f32, 0.0],
                screen: [w as f32 * ppp, h as f32 * ppp, 0.0, 0.0],
                styles: style::table(zf, ppp),
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

            // --- labels (egui) ---
            draw_labels(&painter, rect, &self.cam, zf, &frame_tiles.labels);

            // --- HUD ---
            let lat = self.cam.lat.to_degrees();
            let lon = self.cam.lon.to_degrees();
            egui::Area::new(egui::Id::new("hud")).fixed_pos(rect.min + egui::vec2(10.0, 10.0)).show(ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_black_alpha(170))
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        let t = |s: String| egui::RichText::new(s).monospace().color(egui::Color32::WHITE).size(12.0);
                        ui.label(t(format!("{:.0} fps   zoom {:.2}", self.fps, zf)));
                        ui.label(t(format!("{:.4}°, {:.4}°", lat, lon)));
                        ui.label(t(format!(
                            "tiles: {} drawn, {} cached, {} loading{}",
                            frame_tiles.draw.len(),
                            frame_tiles.loaded,
                            frame_tiles.pending,
                            if frame_tiles.failed > 0 { format!(", {} retrying", frame_tiles.failed) } else { String::new() }
                        )));
                        ui.label(t(format!("GPU: {}", self.gpu_name)));
                        ui.horizontal(|ui| {
                            go_globe = ui.button("Globe").clicked();
                            go_sf = ui.button("San Francisco").clicked();
                            clear_cache = ui.button("Clear cache").clicked();
                        });
                        ui.label(t("drag: pan   wheel: zoom   double-click: zoom in".to_string()));
                    });
            });

            // --- attribution (required by the data licences) ---
            egui::Area::new(egui::Id::new("attribution"))
                .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-8.0, -8.0))
                .show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(egui::Color32::from_black_alpha(170))
                        .corner_radius(4.0)
                        .inner_margin(egui::Margin::symmetric(6, 3))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                ui.hyperlink_to("OpenFreeMap", "https://openfreemap.org");
                                ui.hyperlink_to("© OpenMapTiles", "https://openmaptiles.org/");
                                ui.label("Data from");
                                ui.hyperlink_to("OpenStreetMap", "https://www.openstreetmap.org/copyright");
                            });
                        });
                });
        });

        if go_globe {
            let mut to = self.cam;
            to.alt = 2.2;
            self.fly_to(to, now, 2.5);
        }
        if go_sf {
            let h = ctx.screen_rect().height() as f64;
            let alt = Camera::alt_for_zoom(SF_ZOOM, SF_LAT.to_radians(), h);
            self.fly_to(Camera::from_degrees(SF_LON, SF_LAT, alt), now, 4.5);
        }
        if clear_cache {
            self.tiles.clear_cache();
        }
    }
}

/// Places labels greedily by priority; a label that would overlap one already placed is dropped.
fn draw_labels(painter: &egui::Painter, rect: egui::Rect, cam: &Camera, zf: f64, tiles: &[Arc<Vec<mesh::Label>>]) {
    let (w, h) = (rect.width() as f64, rect.height() as f64);
    let dir = cam.dir();
    let horizon = (1.0 / cam.dist()).clamp(-1.0, 1.0).acos();

    struct Cand<'a> {
        priority: f32,
        label: &'a mesh::Label,
        rule: style::LabelRule,
        at: egui::Pos2,
    }
    let mut cands: Vec<Cand> = Vec::new();
    for t in tiles {
        for l in t.iter() {
            let Some(rule) = style::label_rule(l.kind, l.rank, zf) else { continue };
            let p = merc_to_vec(l.mx, l.my);
            let ang = dot(p, dir).clamp(-1.0, 1.0).acos();
            if ang > horizon * 0.92 {
                continue;
            }
            let Some(s) = cam.project(p, w, h) else { continue };
            if s[0] < -40.0 || s[0] > w + 40.0 || s[1] < -20.0 || s[1] > h + 20.0 {
                continue;
            }
            cands.push(Cand {
                priority: rule.priority,
                label: l,
                rule,
                at: rect.min + egui::vec2(s[0] as f32, s[1] as f32),
            });
        }
    }
    cands.sort_by(|a, b| a.priority.partial_cmp(&b.priority).unwrap_or(std::cmp::Ordering::Equal));

    let mut placed: Vec<egui::Rect> = Vec::new();
    for c in cands.into_iter().take(500) {
        if placed.len() >= 160 {
            break;
        }
        let text = if c.rule.upper { c.label.text.to_uppercase() } else { c.label.text.clone() };
        let font = egui::FontId::proportional(c.rule.size);
        let col = egui::Color32::from_rgb(c.rule.color[0], c.rule.color[1], c.rule.color[2]);
        let halo = egui::Color32::from_rgb(c.rule.halo[0], c.rule.halo[1], c.rule.halo[2]);
        let main = painter.layout_no_wrap(text.clone(), font.clone(), col);
        let size = main.size();
        let r = egui::Rect::from_center_size(c.at, size);
        let padded = r.expand2(egui::vec2(4.0, 2.0));
        if placed.iter().any(|p| p.intersects(padded)) {
            continue;
        }
        placed.push(padded);

        let shadow = painter.layout_no_wrap(text, font, halo);
        let o = 1.3;
        for (dx, dy) in [(-o, 0.0), (o, 0.0), (0.0, -o), (0.0, o), (-o, -o), (o, -o), (-o, o), (o, o)] {
            painter.galley(r.min + egui::vec2(dx, dy), shadow.clone(), halo);
        }
        painter.galley(r.min, main, col);
    }
}
