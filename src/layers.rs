//! The Layers button's panel (transit map and GPS paths on or off) and the "Transit here" list in
//! the upper right: drag the grips to change which system, or which line, is drawn on top, and
//! open a system like a folder to switch its lines on or off one by one.

use eframe::egui::{self, Color32, RichText, Sense, Shape, Stroke};

use std::sync::Arc;

use crate::phonesync::{self, PhoneSync};
use crate::detail;
use crate::tracks;
use crate::transit;
use crate::App;

/// What the pointer is carrying in the "Transit here" list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dnd {
    /// A whole system (its slot).
    System(usize),
    /// One line of a system: (slot, line index).
    Line(usize, usize),
}

/// Three little stacked rhombi: the usual "layers" symbol, painted so it needs no icon font.
pub fn layers_icon(ui: &mut egui::Ui, size: egui::Vec2, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let vis = ui.visuals();
    if on || resp.hovered() {
        let bg = if on { vis.selection.bg_fill } else { vis.widgets.hovered.weak_bg_fill };
        ui.painter().rect_filled(rect, 4.0, bg);
    }
    let c = if on { vis.selection.stroke.color } else { vis.text_color() };
    let m = rect.center();
    let (hw, hh, gap) = (size.x * 0.32, size.y * 0.17, size.y * 0.2);
    for i in (0..3).rev() {
        let cy = m.y + (i as f32 - 1.0) * gap + 1.0;
        let pts = vec![egui::pos2(m.x - hw, cy), egui::pos2(m.x, cy - hh), egui::pos2(m.x + hw, cy), egui::pos2(m.x, cy + hh)];
        if i == 0 {
            ui.painter().add(Shape::convex_polygon(pts, c, Stroke::new(1.0_f32, c)));
        } else {
            ui.painter().add(Shape::closed_line(pts, Stroke::new(1.6_f32, c)));
        }
    }
    resp
}

/// A small triangle that points right (closed) or down (open).
fn fold_arrow(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(16.0, 18.0), Sense::click());
    let c = if resp.hovered() { ui.visuals().strong_text_color() } else { ui.visuals().text_color() };
    let m = rect.center();
    let s = 4.5;
    let pts = if open {
        vec![m + egui::vec2(-s, -s * 0.6), m + egui::vec2(s, -s * 0.6), m + egui::vec2(0.0, s * 0.8)]
    } else {
        vec![m + egui::vec2(-s * 0.6, -s), m + egui::vec2(s * 0.8, 0.0), m + egui::vec2(-s * 0.6, s)]
    };
    ui.painter().add(Shape::convex_polygon(pts, c, Stroke::NONE));
    resp
}

/// Three short bars: the handle to drag a row by.
fn grip(ui: &mut egui::Ui) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(16.0, 18.0), Sense::drag());
    let c = if resp.hovered() || resp.dragged() { ui.visuals().strong_text_color() } else { ui.visuals().weak_text_color() };
    for k in -1..=1 {
        let y = rect.center().y + k as f32 * 4.0;
        ui.painter().line_segment([egui::pos2(rect.min.x + 2.0, y), egui::pos2(rect.max.x - 2.0, y)], Stroke::new(1.6_f32, c));
    }
    resp.on_hover_cursor(egui::CursorIcon::Grab)
}

pub(crate) fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// Where a dragged row lands: the number of rows whose middle is above the pointer.
fn insert_index(rows: &[egui::Rect], y: f32) -> usize {
    rows.iter().filter(|r| r.center().y < y).count()
}

/// The row order after moving the row at `from` to the gap `at` (0 = before the first row).
fn moved(list: &[usize], from: usize, at: usize) -> Vec<usize> {
    let mut v = list.to_vec();
    let item = v.remove(from);
    let to = if at > from { at - 1 } else { at };
    v.insert(to.min(v.len()), item);
    v
}

/// A line across the gap before row `at`, so the drop spot is visible.
fn gap_line(ui: &egui::Ui, rows: &[egui::Rect], at: usize) {
    let (Some(first), Some(last)) = (rows.first(), rows.last()) else { return };
    let y = if at == 0 { first.min.y - 1.0 } else if at >= rows.len() { last.max.y + 1.0 } else { (rows[at - 1].max.y + rows[at].min.y) * 0.5 };
    let (x0, x1) = (first.min.x, first.max.x);
    ui.painter().line_segment([egui::pos2(x0, y), egui::pos2(x1, y)], Stroke::new(2.5_f32, Color32::from_rgb(255, 210, 60)));
}

impl App {
    /// Starts or stops listening for the phone tracker.
    pub(crate) fn set_phone_sync(&mut self, ctx: &egui::Context, on: bool) {
        self.phone_sync_on = on;
        if on && self.phone_sync.is_none() {
            let c = ctx.clone();
            // The pairing file lives in the permanent per-user folder, not in the `data` folder that
            // comes with each build; one kept there by an older version is copied over.
            let cfg = crate::data::user_data_dir().join("sync.txt");
            let old = self.data_dir.join("sync.txt");
            if !cfg.exists() && old.is_file() {
                let _ = std::fs::create_dir_all(crate::data::user_data_dir());
                let _ = std::fs::copy(&old, &cfg);
            }
            self.phone_sync = Some(PhoneSync::start(
                self.tracks.dir().to_path_buf(),
                cfg,
                phonesync::TCP_PORT,
                phonesync::UDP_PORT,
                Arc::new(move || c.request_repaint()),
            ));
        } else if !on {
            self.phone_sync = None; // dropping it stops the listeners
        }
    }

    /// "Map details": a switch for every kind of thing the map tiles hold.
    fn map_detail_section(&mut self, ui: &mut egui::Ui) {
        let off = detail::ITEMS.iter().enumerate().filter(|(i, _)| !self.detail.get(*i)).count();
        let title = if off == 0 { "Map details".to_string() } else { format!("Map details ({off} hidden)") };
        egui::CollapsingHeader::new(title).id_salt("map_details").default_open(false).show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.small_button("Show all").clicked() {
                    self.detail.set_all(None, true);
                }
                if ui.small_button("Hide all").clicked() {
                    self.detail.set_all(None, false);
                }
            });
            egui::ScrollArea::vertical().max_height(340.0).auto_shrink([true, true]).show(ui, |ui| {
                let mut group = "";
                for (i, it) in detail::ITEMS.iter().enumerate() {
                    if it.group != group {
                        group = it.group;
                        ui.add_space(3.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(group).strong());
                            if ui.small_button("all").clicked() {
                                self.detail.set_all(Some(group), true);
                            }
                            if ui.small_button("none").clicked() {
                                self.detail.set_all(Some(group), false);
                            }
                        });
                    }
                    ui.checkbox(self.detail.flag_mut(i), it.name);
                }
            });
            ui.small("Takes effect at once; nothing is downloaded again. Places of interest, house numbers and names appear as you zoom in.");
        });
    }

    /// The panel under the Layers button.
    pub(crate) fn layers_popup(&mut self, ctx: &egui::Context, at: egui::Pos2, button: egui::Rect, now: f64) {
        if !self.layers_open {
            return;
        }
        let mut go: Option<usize> = None;
        let mut sync_toggle: Option<bool> = None;
        let mut forget_phone: Option<String> = None;
        let resp = egui::Area::new(egui::Id::new("layers_panel")).order(egui::Order::Foreground).fixed_pos(at).show(ctx, |ui| {
            egui::Frame::window(&ctx.style()).fill(Color32::from_black_alpha(235)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                ui.set_width(330.0);
                ui.label(RichText::new("Layers").strong());
                ui.separator();
                self.map_detail_section(ui);
                ui.add_space(4.0);
                ui.checkbox(&mut self.show_transit, "Transit map");
                ui.add_enabled_ui(self.show_transit, |ui| {
                    ui.indent("layers_transit", |ui| {
                        ui.checkbox(&mut self.show_trains, "Moving trains (simulated)");
                    });
                });
                ui.add_space(4.0);
                ui.checkbox(&mut self.show_tracks, "GPS paths");
                ui.add_enabled_ui(self.show_tracks, |ui| {
                    ui.indent("layers_tracks", |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Show");
                            for (v, name) in tracks::View::ALL {
                                ui.selectable_value(&mut self.track_view, v, name);
                            }
                        });
                        ui.small("Cleaned up drops stray jumps and smooths the wobble. Snapped to streets then pulls it onto the street network from the map tiles on your disk (parks and places with no street stay as they were). The original is never changed. Click a path on the map for time, distance and replay.");
                        if self.track_view == tracks::View::Snapped {
                            let line = self.tracks.snap_status();
                            if !line.is_empty() {
                                ui.small(line);
                            }
                            if self.tracks.snap_incomplete() && ui.small_button("Redo").on_hover_text("Snap again, now that the map tiles have been viewed").clicked() {
                                self.tracks.resnap();
                            }
                        }
                        if self.tracks.items.is_empty() {
                            ui.small("No tracks yet. Drop a .kml file onto this window to add one.");
                        } else {
                            egui::ScrollArea::vertical().max_height(230.0).auto_shrink([true, true]).show(ui, |ui| {
                                for (i, it) in self.tracks.items.iter_mut().enumerate() {
                                    ui.horizontal(|ui| {
                                        let c = it.track.color;
                                        let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 14.0), Sense::hover());
                                        ui.painter().rect_filled(r, 3.0, Color32::from_rgb(c[0], c[1], c[2]));
                                        ui.checkbox(&mut it.shown, format!("{}  ·  {:.1} km", clip(&it.track.name, 26), it.track.km));
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if ui.small_button("Go").on_hover_text("Fly to this track").clicked() {
                                                go = Some(i);
                                            }
                                        });
                                    });
                                }
                            });
                        }
                        if !self.tracks.message.is_empty() {
                            ui.small(self.tracks.message.clone());
                        }
                        ui.small(format!("Tracks are kept in {}", self.tracks.dir().display()));
                    });
                });
                ui.add_space(4.0);
                let mut sync_on = self.phone_sync_on;
                if ui.checkbox(&mut sync_on, "Phone sync (tracks arrive over WiFi)").changed() {
                    sync_toggle = Some(sync_on);
                }
                if let Some(sync) = &self.phone_sync {
                    let snap = sync.snapshot();
                    ui.indent("layers_sync", |ui| {
                        if !snap.error.is_empty() {
                            ui.colored_label(Color32::from_rgb(255, 130, 110), snap.error.clone());
                        }
                        let ip = phonesync::local_ip().map(|a| a.to_string()).unwrap_or_else(|| "unknown".into());
                        ui.small(format!("This computer: {} ({})", snap.desk_name, ip));
                        match snap.code {
                            Some(code) => {
                                ui.label(RichText::new(code).monospace().size(28.0).strong());
                                ui.small(format!("Type this code on the phone (open for {} more seconds).", snap.code_secs_left));
                                if ui.button("Cancel pairing").clicked() {
                                    sync.cancel_pairing();
                                }
                                ctx.request_repaint_after(std::time::Duration::from_secs(1));
                            }
                            None => {
                                if ui.button("Pair a phone").clicked() {
                                    sync.begin_pairing();
                                }
                            }
                        }
                        for (id, name) in &snap.phones {
                            ui.horizontal(|ui| {
                                ui.label(format!("Paired: {}", clip(name, 28)));
                                if ui.small_button("Forget").on_hover_text("Stop accepting tracks from this phone").clicked() {
                                    forget_phone = Some(id.clone());
                                }
                            });
                        }
                        for line in snap.log.iter().rev().take(4) {
                            ui.small(clip(line, 52));
                        }
                        ui.small("Windows may ask to allow network access the first time: choose Private networks.");
                    });
                }
            });
        });
        if let Some(on) = sync_toggle {
            self.set_phone_sync(ctx, on);
        }
        if let (Some(id), Some(sync)) = (forget_phone, &self.phone_sync) {
            sync.forget(&id);
        }
        if let Some(i) = go {
            let (lon, lat) = self.tracks.items[i].track.lon_lat_deg();
            let zoom = crate::zoom_for_span(self.tracks.items[i].track.radius * 2.0, lat.to_radians(), self.view_h);
            self.goto_zoom(lon, lat, zoom, now);
        }
        // a click anywhere else closes it
        let pressed = ctx.input(|i| i.pointer.any_pressed());
        if pressed {
            if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
                if !resp.response.rect.contains(p) && !button.contains(p) {
                    self.layers_open = false;
                }
            }
        }
    }

    /// The "Transit here" list (upper right) while the view is zoomed in on a city with transit.
    pub(crate) fn systems_popup(&mut self, ctx: &egui::Context, rect: egui::Rect, zf: f64) {
        if !self.show_transit || zf < transit::MAP_FULL {
            self.dnd = None;
            return;
        }
        let near = self.transit.nearby(&self.cam, (rect.width() / rect.height().max(1.0)) as f64);
        if near.is_empty() {
            self.dnd = None;
            return;
        }
        let released = ctx.input(|i| i.pointer.any_released());
        let pointer_y = ctx.input(|i| i.pointer.latest_pos()).map(|p| p.y);
        if self.dnd.is_some() {
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        let dragging_system = matches!(self.dnd, Some(Dnd::System(_)));
        let mut toggle: Option<(usize, bool)> = None;
        let mut line_hidden: Vec<(usize, usize, bool)> = Vec::new();
        let mut all_lines: Option<(usize, bool)> = None;
        let mut new_systems: Option<Vec<usize>> = None;
        let mut new_lines: Option<(usize, Vec<usize>)> = None;
        let mut started: Option<Dnd> = None;
        let mut system_to_top: Option<usize> = None;
        let mut line_to_top: Option<(usize, usize)> = None;

        egui::Area::new(egui::Id::new("systems_here")).anchor(egui::Align2::RIGHT_TOP, egui::vec2(-10.0, 64.0)).show(ctx, |ui| {
            let frame = egui::Frame::popup(ui.style());
            let pad_bottom = frame.inner_margin.bottom as f32 + frame.stroke.width + 1.0;
            frame.show(ui, |ui| {
                ui.set_width(300.0);
                ui.horizontal(|ui| {
                    if fold_arrow(ui, !self.systems_folded).clicked() {
                        self.systems_folded = !self.systems_folded;
                    }
                    ui.label(RichText::new(format!("Transit here ({})", near.len())).strong());
                });
                if self.systems_folded {
                    return;
                }
                ui.small("Drag a grip to move a row, or press Top. The top row is drawn on top.");
                // the list runs down to the bottom of the window
                let list_h = (rect.max.y - 10.0 - pad_bottom - ui.cursor().top()).max(160.0);
                egui::ScrollArea::vertical().min_scrolled_height(list_h).max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
                    let mut groups: Vec<egui::Rect> = Vec::new();
                    for &slot in &near {
                        let open = self.systems_open.contains(&slot) && !dragging_system;
                        let group = ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                if grip(ui).drag_started() {
                                    started = Some(Dnd::System(slot));
                                }
                                if fold_arrow(ui, open).on_hover_text("Show its lines").clicked() {
                                    if !self.systems_open.remove(&slot) {
                                        self.systems_open.insert(slot);
                                    }
                                }
                                let mut on = self.transit.enabled(slot);
                                if ui.checkbox(&mut on, self.transit.title(slot)).changed() {
                                    toggle = Some((slot, on));
                                }
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    let is_top = near.first() == Some(&slot);
                                    if ui.add_enabled(!is_top, egui::Button::new("Top").small()).on_hover_text("Draw this system on top of the others").clicked() {
                                        system_to_top = Some(slot);
                                    }
                                });
                            });
                            if open {
                                self.lines_list(ui, ctx, slot, released, pointer_y, &mut line_hidden, &mut all_lines, &mut new_lines, &mut started, &mut line_to_top);
                            }
                        });
                        groups.push(group.response.rect);
                    }
                    if let (Some(Dnd::System(slot)), Some(y)) = (self.dnd, pointer_y) {
                        if let Some(from) = near.iter().position(|&s| s == slot) {
                            let at = insert_index(&groups, y);
                            gap_line(ui, &groups, at);
                            if released {
                                new_systems = Some(moved(&near, from, at));
                            }
                        }
                    }
                });
            });
        });

        if let Some(d) = started {
            self.dnd = Some(d);
        }
        if let Some((slot, on)) = toggle {
            self.transit.set_enabled(slot, on);
        }
        for (slot, li, h) in line_hidden {
            self.transit.set_line_hidden(slot, li, h);
        }
        if let Some((slot, hide)) = all_lines {
            if let Some(d) = self.transit.data(slot) {
                for li in 0..d.lines.len() {
                    self.transit.set_line_hidden(slot, li, hide);
                }
            }
        }
        if let Some(slot) = system_to_top {
            let mut order: Vec<usize> = near.iter().copied().filter(|&s| s != slot).collect();
            order.insert(0, slot);
            self.transit.reorder_systems(&order);
        }
        if let Some((slot, li)) = line_to_top {
            let mut order: Vec<usize> = self.transit.prefs(slot).order.iter().rev().copied().filter(|&l| l != li).collect();
            order.insert(0, li);
            self.transit.reorder_lines(slot, &order);
        }
        if let Some(order) = new_systems {
            self.transit.reorder_systems(&order);
        }
        if let Some((slot, order)) = new_lines {
            self.transit.reorder_lines(slot, &order);
        }
        // the drag is over when the button is up
        if released || !ctx.input(|i| i.pointer.any_down()) {
            if started.is_none() {
                self.dnd = None;
            }
        }
    }

    /// The lines of one open system: on/off boxes, and grips to reorder them.
    #[allow(clippy::too_many_arguments)]
    fn lines_list(
        &mut self,
        ui: &mut egui::Ui,
        _ctx: &egui::Context,
        slot: usize,
        released: bool,
        pointer_y: Option<f32>,
        line_hidden: &mut Vec<(usize, usize, bool)>,
        all_lines: &mut Option<(usize, bool)>,
        new_lines: &mut Option<(usize, Vec<usize>)>,
        started: &mut Option<Dnd>,
        line_to_top: &mut Option<(usize, usize)>,
    ) {
        let Some(data) = self.transit.data(slot) else { return };
        let prefs = self.transit.prefs(slot).clone();
        // top of the drawing order first
        let top_first: Vec<usize> = prefs.order.iter().rev().copied().filter(|&li| li < data.lines.len()).collect();
        ui.indent(("lines", slot), |ui| {
            ui.horizontal(|ui| {
                if ui.small_button("Show all").clicked() {
                    *all_lines = Some((slot, false));
                }
                if ui.small_button("Hide all").clicked() {
                    *all_lines = Some((slot, true));
                }
                ui.weak(format!("{} lines", data.lines.len()));
            });
            let mut rows: Vec<egui::Rect> = Vec::new();
            for &li in &top_first {
                let line = &data.lines[li];
                let r = ui.horizontal(|ui| {
                    if grip(ui).drag_started() {
                        *started = Some(Dnd::Line(slot, li));
                    }
                    let c = line.color;
                    let (sw, _) = ui.allocate_exact_size(egui::vec2(12.0, 14.0), Sense::hover());
                    ui.painter().rect_filled(sw, 3.0, Color32::from_rgb(c[0], c[1], c[2]));
                    let mut on = !prefs.is_hidden(li);
                    let name = if line.badge.is_empty() { clip(&line.name, 30) } else { format!("{}  {}", line.badge, clip(&line.name, 26)) };
                    if ui.checkbox(&mut on, name).changed() {
                        line_hidden.push((slot, li, !on));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let is_top = top_first.first() == Some(&li);
                        if ui.add_enabled(!is_top, egui::Button::new("Top").small()).on_hover_text("Draw this line on top of the others in its system").clicked() {
                            *line_to_top = Some((slot, li));
                        }
                    });
                });
                rows.push(r.response.rect);
            }
            if let (Some(Dnd::Line(s, li)), Some(y)) = (self.dnd, pointer_y) {
                if s == slot {
                    if let Some(from) = top_first.iter().position(|&l| l == li) {
                        let at = insert_index(&rows, y);
                        gap_line(ui, &rows, at);
                        if released {
                            *new_lines = Some((slot, moved(&top_first, from, at)));
                        }
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_a_row_lands_in_the_gap_that_was_shown() {
        let l = [10, 11, 12, 13];
        assert_eq!(moved(&l, 0, 0), vec![10, 11, 12, 13], "dropped back where it was");
        assert_eq!(moved(&l, 0, 1), vec![10, 11, 12, 13], "the gap right below itself is also where it was");
        assert_eq!(moved(&l, 0, 3), vec![11, 12, 10, 13]);
        assert_eq!(moved(&l, 0, 4), vec![11, 12, 13, 10], "to the very bottom");
        assert_eq!(moved(&l, 3, 0), vec![13, 10, 11, 12], "to the very top");
        assert_eq!(moved(&l, 2, 1), vec![10, 12, 11, 13]);
    }

    #[test]
    fn insert_index_counts_rows_above_the_pointer() {
        let rows: Vec<egui::Rect> = (0..3).map(|i| egui::Rect::from_min_size(egui::pos2(0.0, i as f32 * 20.0), egui::vec2(100.0, 18.0))).collect();
        assert_eq!(insert_index(&rows, -5.0), 0);
        assert_eq!(insert_index(&rows, 15.0), 1);
        assert_eq!(insert_index(&rows, 100.0), 3);
    }
}
