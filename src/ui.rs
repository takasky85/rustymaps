//! The controls: a slim bar along the top (Places / Map / Transit / Flights / View, a search box
//! and a reset button). Each tab opens one compact panel under the bar, so nothing needs scrolling
//! and every setting sits where you would look for it. Click the tab again, click empty map or
//! press Esc to close it.

use eframe::egui::{self, Color32, RichText};

use crate::camera::{vec_to_lonlat, MAX_PITCH};
use crate::search::Hit;
use crate::style::Palette;
use crate::transit_data::SYSTEMS;
use crate::{App, CITY_ZOOM, LABEL_SIZES, SPEEDS};

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
        // The tab panels reach almost to the bottom of the window.
        let avail_h = (rect.height() - (14.0 + 34.0 * k) - 28.0).max(240.0);

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
                    ui.label(RichText::new(format!("zoom {:.1}", zf)).monospace())
                        .on_hover_text("map zoom level (the same scale as web maps: 0 = whole world, 17+ = street level)");
                    ui.label(
                        RichText::new(format!(
                            "{}/s (input {})  map {}  planes {}",
                            self.rates[0], self.input_rate, self.rates[1], self.rates[2]
                        ))
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
                    ui.set_min_height(avail_h);
                    egui::ScrollArea::vertical().max_height(avail_h).auto_shrink([false, false]).show(ui, |ui| match tab {
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
            ui.add(egui::TextEdit::singleline(&mut self.places_filter).hint_text("Search places…").desired_width(ui.available_width()));
        });
        ui.horizontal(|ui| {
            ui.label("Fly");
            for (i, (name, _)) in crate::fly::FLY_SPEEDS.iter().enumerate() {
                ui.selectable_value(&mut self.fly_speed, i, *name);
            }
        });
        ui.separator();

        let q = self.places_filter.trim().to_lowercase();
        let matches = |i: usize| q.is_empty() || SYSTEMS[i].title.to_lowercase().contains(&q) || SYSTEMS[i].id.contains(&q);
        let mut sorted: Vec<usize> = (0..SYSTEMS.len()).collect();
        sorted.sort_by_key(|&i| SYSTEMS[i].title.to_lowercase());
        let favs: Vec<usize> = sorted.iter().copied().filter(|&i| self.favorites.iter().any(|f| f == SYSTEMS[i].id) && matches(i)).collect();

        let mut goto: Option<usize> = None;
        let mut star: Option<usize> = None;
        let (row_h, k) = (26.0 * self.k(), self.k());
        let mut row = |ui: &mut egui::Ui, i: usize, is_fav: bool| {
            ui.horizontal(|ui| {
                let mark = if is_fav { "★" } else { "☆" };
                let tip = if is_fav { "Remove from favorites" } else { "Add to favorites" };
                let colour = if is_fav { Color32::from_rgb(255, 200, 40) } else { Color32::GRAY };
                if ui.add_sized([row_h, row_h], egui::Button::new(RichText::new(mark).color(colour).size(15.0 * k)).frame(false)).on_hover_text(tip).clicked() {
                    star = Some(i);
                }
                if ui.add_sized([ui.available_width(), row_h], egui::Button::new(SYSTEMS[i].title).truncate()).on_hover_text(SYSTEMS[i].title).clicked() {
                    goto = Some(i);
                }
            });
        };
        if !favs.is_empty() {
            ui.label(RichText::new("Favorites").strong());
            for &i in &favs {
                row(ui, i, true);
            }
            ui.separator();
        }
        let all: Vec<usize> = sorted.iter().copied().filter(|&i| matches(i)).collect();
        ui.label(RichText::new(if q.is_empty() { format!("All places ({})", all.len()) } else { format!("{} found", all.len()) }).strong());
        let mut letter = ' ';
        for &i in &all {
            if q.is_empty() {
                let first = SYSTEMS[i].title.chars().next().map_or(' ', |c| c.to_ascii_uppercase());
                if first != letter {
                    letter = first;
                    ui.add_space(4.0 * k);
                    ui.label(RichText::new(letter.to_string()).small().weak());
                }
            }
            let is_fav = self.favorites.iter().any(|f| f == SYSTEMS[i].id);
            row(ui, i, is_fav);
        }
        if all.is_empty() {
            ui.label("Nothing matches.");
        }
        if let Some(i) = star {
            let id = SYSTEMS[i].id.to_string();
            match self.favorites.iter().position(|f| *f == id) {
                Some(at) => {
                    self.favorites.remove(at);
                }
                None => self.favorites.push(id),
            }
        }
        if let Some(i) = goto {
            self.goto_city(SYSTEMS[i].home.1, SYSTEMS[i].home.0, now);
        }
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
        ui.horizontal(|ui| {
            let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            if ui.button(if full { "Leave full screen (F11)" } else { "Full screen (F11)" }).clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
            }
        });
        ui.checkbox(&mut self.show_compass, "Mini globe (bottom right)");
        ui.checkbox(&mut self.bilingual, "Station names: local script with English underneath");
        ui.checkbox(&mut self.transit_focus, "Subway focus: dim the map, hide its labels");
        if self.transit_focus {
            ui.add(egui::Slider::new(&mut self.map_dim, 0.0..=0.85).text("Dim"));
        }
        ui.checkbox(&mut self.flat_stations, "Station circles lie flat when tilted");
        ui.small("Styles are colour recreations of well-known looks, drawn from the same vector data.");
    }

    // ---------------------------------------------------------------- Transit

    fn tab_transit(&mut self, ui: &mut egui::Ui) {
        let have: Vec<&str> = (0..SYSTEMS.len()).filter(|&i| self.transit.offline(i)).map(|i| SYSTEMS[i].title).collect();
        ui.label(format!("{} city maps are installed. They appear by themselves when you zoom in on the city.", have.len()));
        ui.small("Zoomed out, each city shows a metro symbol; click it to fly there. Click a line to highlight it; click empty map or press Esc to clear.");
        ui.separator();
        ui.checkbox(&mut self.show_trains, "Moving trains (simulated, not a timetable)");
        ui.add_enabled_ui(self.show_trains, |ui| {
            ui.horizontal(|ui| {
                ui.label("Train speed");
                for (i, (name, _)) in crate::TRAIN_SPEEDS.iter().enumerate() {
                    ui.selectable_value(&mut self.train_speed, i, *name);
                }
            });
            ui.small("Real time: about 65 km/h between stops, stopping at every station.");
        });
        ui.separator();
        egui::CollapsingHeader::new("Installed cities").default_open(false).show(ui, |ui| {
            ui.label(have.join(" · "));
        });
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
        ui.horizontal_wrapped(|ui| {
            ui.label("Tile cache limit");
            let before = crate::tiles::cache_cap_mb();
            let mut mb = before;
            for (name, v) in [("256 MB", 256u64), ("512 MB", 512), ("1 GB", 1024), ("2 GB", 2048), ("4 GB", 4096), ("8 GB", 8192)] {
                ui.selectable_value(&mut mb, v, name);
            }
            if mb != before {
                crate::tiles::set_cache_cap_mb(mb);
                if mb < before {
                    self.tiles.trim_cache();
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Graphics API");
            for (i, (name, _)) in crate::BACKENDS.iter().enumerate() {
                ui.selectable_value(&mut self.backend, i, *name);
            }
        });
        ui.small(format!(
            "Applies the next time you start the app. Now running on: {}",
            crate::ACTIVE_GPU.get().map(String::as_str).unwrap_or("unknown")
        ));
        ui.small(format!("Downloaded map tiles are kept in {} (oldest are removed above the limit).", self.tiles.cache_dir().display()));
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
