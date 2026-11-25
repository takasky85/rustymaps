//! The controls: a slim bar along the top (Places / Map / Transit / Flights / View, a search box
//! and a reset button). Each tab opens one compact panel under the bar, so nothing needs scrolling
//! and every setting sits where you would look for it. Click the tab again, click empty map or
//! press Esc to close it.

use eframe::egui::{self, Color32, RichText};

use crate::camera::{vec_to_lonlat, MAX_PITCH};
use crate::search::Hit;
use crate::style::Palette;
use crate::transit::Status;
use crate::transit_data::SYSTEMS;
use crate::{App, CITY_ZOOM, LABEL_SIZES, SF_LAT, SF_LON, SPEEDS};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Places,
    Map,
    Transit,
    Flights,
    View,
}

/// Interface text sizes: (name, multiplier).
pub const UI_SIZES: [(&str, f32); 3] = [("Normal", 1.0), ("Large", 1.25), ("X-large", 1.5)];

/// Sets the interface's text sizes and spacing for the chosen size multiplier.
pub fn apply_scale(ctx: &egui::Context, k: f32) {
    use egui::{FontId, TextStyle};
    ctx.style_mut(|st| {
        st.text_styles.insert(TextStyle::Body, FontId::proportional(15.0 * k));
        st.text_styles.insert(TextStyle::Button, FontId::proportional(15.0 * k));
        st.text_styles.insert(TextStyle::Heading, FontId::proportional(20.0 * k));
        st.text_styles.insert(TextStyle::Small, FontId::proportional(13.0 * k));
        st.text_styles.insert(TextStyle::Monospace, FontId::monospace(14.0 * k));
        st.spacing.item_spacing = egui::vec2(8.0 * k, 6.0 * k);
        st.spacing.button_padding = egui::vec2(8.0 * k, 4.0 * k);
        st.spacing.interact_size.y = 24.0 * k;
        st.spacing.slider_width = 200.0 * k;
        st.spacing.icon_width = 16.0 * k;
        st.spacing.icon_width_inner = 9.0 * k;
    });
}

const TABS: [(Tab, &str); 5] =
    [(Tab::Places, "Places"), (Tab::Map, "Map"), (Tab::Transit, "Transit"), (Tab::Flights, "Flights"), (Tab::View, "View")];

/// Zoom used when flying to a station.
const STATION_ZOOM: f64 = 15.5;

fn frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::window(&ctx.style()).fill(Color32::from_black_alpha(220)).inner_margin(egui::Margin::same(8))
}

fn shorten(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

impl App {
    fn k(&self) -> f32 {
        UI_SIZES[self.ui_size.min(UI_SIZES.len() - 1)].1
    }

    /// Inner width of the tab panels.
    fn panel_w(&self) -> f32 {
        430.0 * self.k()
    }

    /// Width of one cell when `cols` equal cells share a panel row.
    fn cell_w(&self, cols: usize) -> f32 {
        (self.panel_w() - (cols as f32 - 1.0) * 8.0 * self.k()) / cols as f32
    }

    pub(crate) fn controls(&mut self, ctx: &egui::Context, rect: egui::Rect, now: f64, zf: f64) {
        let k = self.k();
        let avail_h = (rect.height() - 80.0 * k).max(240.0);

        // ---- the bar ----
        egui::Area::new(egui::Id::new("bar")).anchor(egui::Align2::LEFT_TOP, egui::vec2(8.0, 8.0)).order(egui::Order::Foreground).show(ctx, |ui| {
            frame(ctx).show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (tab, name) in TABS {
                        let on = self.tab == Some(tab);
                        if ui.selectable_label(on, name).clicked() {
                            self.tab = if on { None } else { Some(tab) };
                            self.search_open = false;
                        }
                    }
                    ui.separator();
                    let resp = ui.add(egui::TextEdit::singleline(&mut self.search_text).hint_text("Search a place or station…").desired_width(220.0 * k));
                    let go = ui.button("Search").clicked();
                    if (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) || go {
                        self.run_search(ctx);
                    }
                    ui.separator();
                    if ui.button("Reset view").on_hover_text("north up, looking straight down, centred (R)").clicked() {
                        self.reset_orientation(now);
                    }
                    ui.label(
                        RichText::new(format!("{}/s  map {}  planes {}", self.rates[0], self.rates[1], self.rates[2]))
                            .monospace()
                            .weak(),
                    )
                    .on_hover_text("frames drawn per second, and how many of them redrew the map / the planes on the GPU");
                });
            });
        });

        // ---- the panel under it ----
        let pos = egui::vec2(8.0, 14.0 + 34.0 * k);
        if self.search_open {
            self.search_panel(ctx, pos, now);
        } else if let Some(tab) = self.tab {
            egui::Area::new(egui::Id::new("panel")).anchor(egui::Align2::LEFT_TOP, pos).order(egui::Order::Foreground).show(ctx, |ui| {
                frame(ctx).show(ui, |ui| {
                    ui.set_width(self.panel_w());
                    egui::ScrollArea::vertical().max_height(avail_h).auto_shrink([true, true]).show(ui, |ui| match tab {
                        Tab::Places => self.tab_places(ui, now),
                        Tab::Map => self.tab_map(ctx, ui),
                        Tab::Transit => self.tab_transit(ui),
                        Tab::Flights => self.tab_flights(ui, now),
                        Tab::View => self.tab_view(ui, now, zf),
                    });
                });
            });
        }
    }

    // ---------------------------------------------------------------- Places

    fn tab_places(&mut self, ui: &mut egui::Ui, now: f64) {
        ui.horizontal(|ui| {
            if ui.button("Whole globe").clicked() {
                let mut to = self.cam;
                to.alt = 2.2;
                to.pitch = 0.0;
                to.heading = 0.0;
                to.shift = [0.0, 0.0];
                self.fly_to(to, now, 2.5);
            }
            if ui.button("San Francisco").clicked() {
                self.goto_city(SF_LON, SF_LAT, now);
            }
        });
        ui.separator();
        ui.label(RichText::new("Cities with a subway map").strong());
        let (cw, ch) = (self.cell_w(3), 28.0 * self.k());
        egui::Grid::new("places").num_columns(3).spacing([8.0 * self.k(), 6.0 * self.k()]).show(ui, |ui| {
            for (n, sys) in SYSTEMS.iter().enumerate() {
                if ui.add_sized([cw, ch], egui::Button::new(sys.title).truncate()).on_hover_text(sys.title).clicked() {
                    self.goto_city(sys.home.1, sys.home.0, now);
                }
                if n % 3 == 2 {
                    ui.end_row();
                }
            }
        });
    }

    // ---------------------------------------------------------------- Map

    fn tab_map(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        ui.label(RichText::new("Style").strong());
        let (cw, ch) = (self.cell_w(3), 34.0 * self.k());
        egui::Grid::new("styles").num_columns(3).spacing([8.0 * self.k(), 6.0 * self.k()]).show(ui, |ui| {
            for (n, p) in Palette::all().enumerate() {
                let r = ui.add_sized([cw, ch], egui::Button::new(p.name()).truncate().selected(self.palette == p)).on_hover_text(p.name());
                // a strip of the style's colours along the bottom edge of the button
                let sw = p.swatch();
                let strip = egui::Rect::from_min_max(egui::pos2(r.rect.min.x + 3.0, r.rect.max.y - 7.0), egui::pos2(r.rect.max.x - 3.0, r.rect.max.y - 3.0));
                let seg = strip.width() / sw.len() as f32;
                for (i, c) in sw.iter().enumerate() {
                    let col = Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, *c as u8);
                    let q = egui::Rect::from_min_size(egui::pos2(strip.min.x + seg * i as f32, strip.min.y), egui::vec2(seg, strip.height()));
                    ui.painter().rect_filled(q, 0.0, col);
                }
                if r.clicked() {
                    self.palette = p;
                }
                if n % 3 == 2 {
                    ui.end_row();
                }
            }
        });
        ui.separator();
        ui.checkbox(&mut self.show_labels, "Labels");
        ui.add_enabled_ui(self.show_labels, |ui| {
            ui.horizontal(|ui| {
                ui.label("Size");
                for (i, (name, _)) in LABEL_SIZES.iter().enumerate() {
                    ui.selectable_value(&mut self.label_size, i, *name);
                }
            });
            ui.checkbox(&mut self.label_bold, "Bold");
            ui.label("Font");
            let names = self.fonts.names();
            let before = self.fonts.selected;
            ui.horizontal_wrapped(|ui| {
                for (i, n) in names.iter().enumerate() {
                    ui.selectable_value(&mut self.fonts.selected, i, *n);
                }
            });
            if self.fonts.selected != before {
                self.fonts.apply(ctx);
            }
            ui.small(if self.fonts.has_real_bold() { "Fonts installed on this computer; this one has a real bold face." } else { "Fonts installed on this computer; bold is faked for this one (pick another for real bold)." });
        });
        ui.separator();
        ui.checkbox(&mut self.show_compass, "Compass");
        ui.checkbox(&mut self.transit_focus, "Subway focus: dim the map, hide its labels");
        if self.transit_focus {
            ui.add(egui::Slider::new(&mut self.map_dim, 0.0..=0.85).text("Dim"));
        }
        ui.checkbox(&mut self.flat_stations, "Station circles lie flat when tilted");
        ui.small("Styles are colour recreations of well-known looks, drawn from the same vector data.");
    }

    // ---------------------------------------------------------------- Transit

    fn transit_row(&mut self, ui: &mut egui::Ui, i: usize) {
        let mut on = self.transit.enabled(i);
        ui.horizontal(|ui| {
            if ui.checkbox(&mut on, SYSTEMS[i].title).changed() {
                self.transit.set_enabled(i, on);
            }
            match self.transit.status(i) {
                Status::Loading => {
                    ui.spinner();
                }
                Status::Failed(e) => {
                    ui.colored_label(Color32::LIGHT_RED, "failed").on_hover_text(e);
                }
                _ => {}
            }
        });
    }

    fn tab_transit(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.show_trains, "Moving trains (simulated, not a timetable)");
        ui.small("Click a line to highlight it. Click empty map, or press Esc, to clear.");
        ui.separator();
        ui.label(RichText::new("Saved on this computer").strong());
        let saved: Vec<usize> = (0..SYSTEMS.len()).filter(|&i| self.transit.offline(i)).collect();
        if saved.is_empty() {
            ui.weak("none yet: run the data workflow, or switch one on below");
        }
        egui::Grid::new("tr_saved").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            for (n, &i) in saved.iter().enumerate() {
                self.transit_row(ui, i);
                if n % 2 == 1 {
                    ui.end_row();
                }
            }
        });
        let more: Vec<usize> = (0..SYSTEMS.len()).filter(|&i| !self.transit.offline(i)).collect();
        if !more.is_empty() {
            egui::CollapsingHeader::new(format!("More cities ({}), downloaded when switched on", more.len())).default_open(false).show(ui, |ui| {
                egui::Grid::new("tr_more").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
                    for (n, &i) in more.iter().enumerate() {
                        self.transit_row(ui, i);
                        if n % 2 == 1 {
                            ui.end_row();
                        }
                    }
                });
            });
        }
        ui.small(format!("Maps are read from {}", self.transit.dir().join("transit").display()));
    }

    // ---------------------------------------------------------------- Flights

    fn tab_flights(&mut self, ui: &mut egui::Ui, now: f64) {
        ui.horizontal(|ui| {
            ui.label("Speed");
            for (i, (name, _)) in SPEEDS.iter().enumerate() {
                ui.selectable_value(&mut self.speed, i, *name);
            }
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
        egui::CollapsingHeader::new("Route lines & quality").default_open(false).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Colour");
                ui.color_edit_button_srgb(&mut self.route_color);
            });
            ui.add(egui::Slider::new(&mut self.route_alpha, 0.1..=1.0).text("Opacity"));
            ui.add(egui::Slider::new(&mut self.route_width, 1.0..=10.0).text("Width"));
            ui.checkbox(&mut self.route_at_altitude, "Line at the plane's altitude");
            ui.horizontal(|ui| {
                ui.label("Plane edges");
                ui.selectable_value(&mut self.plane_quality, 0, "Smooth");
                ui.selectable_value(&mut self.plane_quality, 1, "Fast");
            });
        });
        if !self.model_errors.is_empty() {
            ui.colored_label(Color32::LIGHT_RED, "Some aircraft models were not found:");
            for e in &self.model_errors {
                ui.small(RichText::new(e).color(Color32::LIGHT_RED));
            }
            ui.small(format!("Put the data folder next to the program or at {}", self.data_dir.display()));
        }
        let slider_w = self.panel_w() - 24.0;
        let mut look: Option<usize> = None;
        for i in 0..self.planes.len() {
            ui.separator();
            let p = &mut self.planes[i];
            let st = p.sim.state(&p.route);
            let status = if p.sim.pause > 0.0 {
                "turning around"
            } else if p.sim.outbound {
                "outbound"
            } else {
                "returning"
            };
            ui.horizontal(|ui| {
                ui.checkbox(&mut p.show, format!("{}   {} → {}", p.def.label, p.def.from.0, p.def.to.0));
                if p.show {
                    ui.label(
                        RichText::new(format!("{:.0}%  {:.1} h  {:.0} ft  {}", p.sim.t * 100.0, p.sim.t * p.route.duration_s() / 3600.0, st.alt_m / 0.3048, status))
                            .monospace()
                            .weak(),
                    );
                }
            });
            if !p.show {
                continue;
            }
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
            ui.spacing_mut().slider_width = slider_w;
            let mut tt = p.sim.t;
            if ui.add(egui::Slider::new(&mut tt, 0.0..=1.0).show_value(false)).changed() {
                p.sim.set_t(tt);
            }
        }
        if let Some(i) = look {
            self.look_at_plane(i, now);
        }
        ui.small("Switch a plane on to see it and its route. Altitude × lifts planes above the ground so they read as flying.");
    }

    // ---------------------------------------------------------------- View

    fn tab_view(&mut self, ui: &mut egui::Ui, now: f64, zf: f64) {
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
            if ui.button("Reset view").clicked() {
                self.reset_orientation(now);
            }
            if ui.button("Centre").on_hover_text("put the middle of the view back in the middle of the window").clicked() {
                let mut to = self.cam;
                to.shift = [0.0, 0.0];
                self.fly_to(to, now, 0.7);
            }
            if ui.button("Clear tile cache").clicked() {
                self.tiles.clear_cache();
            }
        });
        ui.horizontal(|ui| {
            ui.label("Text size");
            let before = self.ui_size;
            for (i, (name, _)) in UI_SIZES.iter().enumerate() {
                ui.selectable_value(&mut self.ui_size, i, *name);
            }
            if self.ui_size != before {
                apply_scale(ui.ctx(), self.k());
            }
        });
        ui.horizontal(|ui| {
            ui.label("Animation rate");
            ui.selectable_value(&mut self.anim_fps, 15, "15 fps (lightest)");
            ui.selectable_value(&mut self.anim_fps, 30, "30 fps (light)");
            ui.selectable_value(&mut self.anim_fps, 60, "60 fps");
        });
        ui.small("The map only redraws when something changes; planes and trains pace themselves to this rate.");
        ui.separator();
        let t = |s: String| RichText::new(s).monospace();
        ui.label(t(format!("zoom {:.2}   tilt {:.0}°   rot {:.0}°", zf, self.cam.pitch.to_degrees(), self.cam.heading.to_degrees())));
        ui.label(t(format!("{:.4}°, {:.4}°", self.cam.lat.to_degrees(), self.cam.lon.to_degrees())));
        ui.label(t(format!(
            "tiles: {} drawn, {} cached, {} loading{}",
            self.stats.drawn,
            self.stats.loaded,
            self.stats.pending,
            if self.stats.failed > 0 { format!(", {} retrying", self.stats.failed) } else { String::new() }
        )));
        ui.label(t(format!("GPU: {}", self.gpu_name)));
        ui.separator();
        ui.label(RichText::new("Mouse").strong());
        ui.small("Left-drag: move the map");
        ui.small("Shift/Ctrl + left-drag: slide the whole globe around the window");
        ui.small("Right/middle-drag: tilt (up/down) and rotate (left/right)");
        ui.small("Wheel: zoom   Double-click: zoom in   R: reset   Esc: close panels");
        ui.small(format!("Settings are remembered in {}", crate::settings::path().display()));
    }

    // ---------------------------------------------------------------- Search

    /// Looks up the text in the search box: switched-on subway stations and cities right away,
    /// places through the online search.
    pub(crate) fn run_search(&mut self, ctx: &egui::Context) {
        let q = self.search_text.trim().to_string();
        if q.len() < 2 {
            return;
        }
        let ql = q.to_lowercase();
        let mut local: Vec<Hit> = Vec::new();
        for sys in SYSTEMS.iter().filter(|s| s.title.to_lowercase().contains(&ql)) {
            local.push(Hit {
                name: sys.title.to_string(),
                detail: "subway map".into(),
                lon: sys.home.1,
                lat: sys.home.0,
                span_rad: 0.0,
                zoom: Some(CITY_ZOOM),
            });
        }
        for (name, system, pos) in self.transit.find_stations(&q, 8) {
            let (lon, lat) = vec_to_lonlat(pos);
            local.push(Hit { name, detail: format!("station · {system}"), lon: lon.to_degrees(), lat: lat.to_degrees(), span_rad: 0.0, zoom: Some(STATION_ZOOM) });
        }
        self.local_hits = local;
        self.search.start(ctx, &q);
        self.search_open = true;
        self.tab = None;
    }

    fn search_panel(&mut self, ctx: &egui::Context, pos: egui::Vec2, now: f64) {
        let mut chosen: Option<Hit> = None;
        let mut close = false;
        egui::Area::new(egui::Id::new("search_panel")).anchor(egui::Align2::LEFT_TOP, pos).order(egui::Order::Foreground).show(ctx, |ui| {
            frame(ctx).show(ui, |ui| {
                ui.set_width(self.panel_w());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Search results").strong());
                    if self.search.busy {
                        ui.spinner();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("✕").clicked() {
                            close = true;
                        }
                    });
                });
                for hit in self.local_hits.iter().chain(self.search.results.iter()) {
                    let text = if hit.detail.is_empty() { hit.name.clone() } else { format!("{}  ·  {}", shorten(&hit.name, 34), shorten(&hit.detail, 36)) };
                    if ui.add(egui::Button::new(text).frame(false)).clicked() {
                        chosen = Some(hit.clone());
                    }
                }
                if !self.search.message.is_empty() {
                    ui.weak(&self.search.message);
                }
                if self.local_hits.is_empty() && self.search.results.is_empty() && !self.search.busy && self.search.message.is_empty() {
                    ui.weak("nothing found");
                }
            });
        });
        if let Some(h) = chosen {
            let zoom = h.zoom.unwrap_or_else(|| crate::zoom_for_span(h.span_rad, h.lat.to_radians(), self.view_h));
            self.goto_zoom(h.lon, h.lat, zoom, now);
            self.search_open = false;
        }
        if close {
            self.search_open = false;
        }
    }
}
