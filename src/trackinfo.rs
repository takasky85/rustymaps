//! The card that opens when a GPS path is clicked: time, distance and speed of the walk, a graph of
//! the speed along it, and play / pause / scrub controls for replaying it on the map.

use eframe::egui::{self, Align, Align2, Color32, FontId, Layout, RichText, Sense, Shape, Stroke};

use crate::layers::clip;
use crate::replay::{distance_text, hms, speed_text, Timing};
use crate::tracks::{Item, Player, View, REPLAY_SPEEDS};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Icon {
    Play,
    Pause,
    Restart,
    Close,
}

/// A small button whose picture is painted, so no icon font is needed.
fn icon_button(ui: &mut egui::Ui, icon: Icon) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(30.0, 26.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, 5.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let c = if resp.hovered() { ui.visuals().strong_text_color() } else { ui.visuals().text_color() };
    let m = rect.center();
    let p = ui.painter();
    match icon {
        Icon::Play => {
            p.add(Shape::convex_polygon(vec![m + egui::vec2(-4.5, -7.5), m + egui::vec2(-4.5, 7.5), m + egui::vec2(7.5, 0.0)], c, Stroke::NONE));
        }
        Icon::Pause => {
            p.rect_filled(egui::Rect::from_center_size(m + egui::vec2(-4.0, 0.0), egui::vec2(4.0, 14.0)), 1.0, c);
            p.rect_filled(egui::Rect::from_center_size(m + egui::vec2(4.0, 0.0), egui::vec2(4.0, 14.0)), 1.0, c);
        }
        Icon::Restart => {
            p.rect_filled(egui::Rect::from_center_size(m + egui::vec2(-6.0, 0.0), egui::vec2(3.0, 14.0)), 1.0, c);
            p.add(Shape::convex_polygon(vec![m + egui::vec2(7.0, -7.0), m + egui::vec2(7.0, 7.0), m + egui::vec2(-3.5, 0.0)], c, Stroke::NONE));
        }
        Icon::Close => {
            let s = Stroke::new(2.0_f32, c);
            p.line_segment([m + egui::vec2(-5.0, -5.0), m + egui::vec2(5.0, 5.0)], s);
            p.line_segment([m + egui::vec2(-5.0, 5.0), m + egui::vec2(5.0, -5.0)], s);
        }
    }
    resp
}

/// Shows the card for the selected track. Returns true when it should be closed.
pub fn card(ctx: &egui::Context, items: &[Item], view: View, player: &mut Player) -> bool {
    let Some(item) = items.get(player.item) else { return true };
    let t = &item.track;
    let tl = t.timeline(view);
    if tl.is_empty() {
        return true;
    }
    let total = tl.total_secs().max(1e-6);
    player.u = player.u.clamp(0.0, total);
    let now = tl.at(player.u);
    let (len_metric, len_imperial) = distance_text(tl.total_m());
    let mut close = false;
    let col = Color32::from_rgb(t.color[0], t.color[1], t.color[2]);

    egui::Area::new(egui::Id::new("track_card")).order(egui::Order::Foreground).anchor(Align2::CENTER_BOTTOM, egui::vec2(0.0, -14.0)).show(ctx, |ui| {
        egui::Frame::window(&ctx.style()).fill(Color32::from_black_alpha(238)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.set_width(400.0);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 16.0), Sense::hover());
                ui.painter().rect_filled(r, 3.0, col);
                ui.label(RichText::new(clip(&t.name, 34)).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icon_button(ui, Icon::Close).on_hover_text("Close (Esc)").clicked() {
                        close = true;
                    }
                });
            });
            // totals
            let mut line = format!("{len_metric} ({len_imperial})  ·  {} moving", hms(total));
            match tl.timing {
                Timing::Exact => {
                    let (kmh, mph) = speed_text(tl.avg_ms());
                    let (top, _) = speed_text(tl.max_ms());
                    line.push_str(&format!("  ·  average {kmh} km/h ({mph}), top {top}"));
                }
                Timing::Estimated => {
                    let (kmh, _) = speed_text(tl.avg_ms());
                    line.push_str(&format!("  ·  about {kmh} km/h on average"));
                }
                Timing::None => {}
            }
            ui.small(line);
            ui.add_space(4.0);

            // what the walker is doing right now
            ui.columns(3, |cols| {
                cols[0].label(RichText::new(hms(now.elapsed)).size(24.0).strong());
                cols[0].small(format!("of {}", hms(total)));
                let (here, there) = distance_text(now.dist_m);
                cols[1].label(RichText::new(here).size(24.0).strong());
                cols[1].small(there);
                match tl.timing {
                    Timing::None => {
                        cols[2].label(RichText::new("-").size(24.0).strong());
                        cols[2].small("no times in file");
                    }
                    timing => {
                        let (kmh, mph) = speed_text(now.speed_ms);
                        let mark = if timing == Timing::Estimated { "~" } else { "" };
                        cols[2].label(RichText::new(format!("{mark}{kmh}")).size(24.0).strong());
                        cols[2].small(format!("km/h  ·  {mph}"));
                    }
                }
            });
            ui.add_space(6.0);

            // speed along the walk, and the place in it: click or drag to move
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 58.0), Sense::click_and_drag());
            if resp.clicked() || resp.dragged() {
                if let Some(p) = resp.interact_pointer_pos() {
                    let f = ((p.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0);
                    player.u = f as f64 * total;
                }
            }
            let painter = ui.painter();
            painter.rect_filled(rect, 4.0, Color32::from_white_alpha(14));
            let x_now = rect.min.x + rect.width() * (player.u / total) as f32;
            if tl.timing == Timing::Exact {
                let top = tl.max_ms().max(0.5);
                let step = 2.0_f32;
                let bars = (rect.width() / step) as usize;
                for k in 0..bars {
                    let f = (k as f64 + 0.5) / bars as f64;
                    let v = tl.at(f * total).speed_ms;
                    let h = ((v / top) as f32).clamp(0.0, 1.0) * (rect.height() - 8.0);
                    let x = rect.min.x + k as f32 * step;
                    let a = if x < x_now { 235 } else { 90 };
                    let bar = egui::Rect::from_min_max(egui::pos2(x, rect.max.y - 3.0 - h), egui::pos2(x + step - 0.5, rect.max.y - 3.0));
                    painter.rect_filled(bar, 0.0, Color32::from_rgba_unmultiplied(t.color[0], t.color[1], t.color[2], a));
                }
                painter.text(rect.min + egui::vec2(6.0, 3.0), Align2::LEFT_TOP, format!("speed, top {} km/h", speed_text(top).0), FontId::proportional(10.0), Color32::from_white_alpha(140));
            } else {
                // no speeds to draw: just a progress bar
                let bar = egui::Rect::from_min_max(egui::pos2(rect.min.x, rect.center().y - 3.0), egui::pos2(rect.max.x, rect.center().y + 3.0));
                painter.rect_filled(bar, 3.0, Color32::from_white_alpha(30));
                painter.rect_filled(egui::Rect::from_min_max(bar.min, egui::pos2(x_now, bar.max.y)), 3.0, col);
            }
            painter.line_segment([egui::pos2(x_now, rect.min.y + 1.0), egui::pos2(x_now, rect.max.y - 1.0)], Stroke::new(2.0_f32, Color32::WHITE));

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let playing = player.playing;
                if icon_button(ui, if playing { Icon::Pause } else { Icon::Play }).on_hover_text(if playing { "Pause" } else { "Play" }).clicked() {
                    if playing {
                        player.playing = false;
                    } else {
                        if player.u >= total - 1e-6 {
                            player.u = 0.0;
                        }
                        player.playing = true;
                    }
                }
                if icon_button(ui, Icon::Restart).on_hover_text("Back to the start").clicked() {
                    player.u = 0.0;
                }
                ui.add_space(4.0);
                for &m in REPLAY_SPEEDS.iter() {
                    if ui.selectable_label(player.mult == m, format!("{m}x")).on_hover_text("Replay speed").clicked() {
                        player.mult = m;
                    }
                }
                ui.add_space(4.0);
                ui.checkbox(&mut player.follow, "Follow");
            });
            let note = match tl.timing {
                Timing::Exact => "",
                Timing::Estimated => "This file has no time for each point, so the speed shown is the average (worked out from the times in the track's name). Newer tracker versions record every point's time.",
                Timing::None => "This file has no times, so the replay assumes a walking pace.",
            };
            let which = if view.uses_clean() { "Cleaned-up path." } else { "Original path." };
            ui.small(format!("{which} {note}"));
        });
    });
    close
}
