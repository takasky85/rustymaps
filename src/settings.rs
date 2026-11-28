//! Remembers the view settings between runs in a small `key=value` text file in the user data
//! folder (`settings.txt`). Plain text, std only; a missing or damaged file just means defaults.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;

use eframe::egui;

use crate::style::Palette;
use crate::{App, SPEEDS};

pub fn path() -> PathBuf {
    crate::data::user_data_dir().join("settings.txt")
}

#[derive(Default)]
pub struct Store {
    map: BTreeMap<String, String>,
}

impl Store {
    pub fn parse(text: &str) -> Store {
        let mut map = BTreeMap::new();
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                map.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        Store { map }
    }

    pub fn load() -> Store {
        std::fs::read_to_string(path()).map(|t| Store::parse(&t)).unwrap_or_default()
    }

    pub fn set(&mut self, key: &str, value: impl ToString) {
        self.map.insert(key.to_string(), value.to_string());
    }

    pub fn get<T: FromStr>(&self, key: &str) -> Option<T> {
        self.map.get(key).and_then(|v| v.parse().ok())
    }

    pub fn text(&self) -> String {
        let mut s = String::from("# RustyMaps settings (edited by the program; safe to delete)\n");
        for (k, v) in &self.map {
            s.push_str(&format!("{k}={v}\n"));
        }
        s
    }
}

impl App {
    /// The current settings as file text.
    pub(crate) fn settings_text(&self) -> String {
        let mut s = Store::default();
        s.set("palette", self.palette.name());
        s.set("labels", self.show_labels);
        s.set("label_size", self.label_size);
        s.set("label_bold", self.label_bold);
        s.set("font", self.fonts.names()[self.fonts.selected.min(self.fonts.names().len() - 1)]);
        s.set("compass", self.show_compass);
        s.set("focus", self.transit_focus);
        s.set("dim", self.map_dim);
        s.set("flat_stations", self.flat_stations);
        s.set("trains", self.show_trains);
        s.set("route_r", self.route_color[0]);
        s.set("route_g", self.route_color[1]);
        s.set("route_b", self.route_color[2]);
        s.set("route_alpha", self.route_alpha);
        s.set("route_width", self.route_width);
        s.set("route_alt", self.route_at_altitude);
        s.set("speed", self.speed);
        s.set("plane_size", self.plane_size);
        s.set("exaggeration", self.exaggeration);
        s.set("plane_quality", self.plane_quality);
        s.set("anim_fps", self.anim_fps);
        s.set("train_speed", self.train_speed);
        s.set("backend", self.backend);
        s.set("fly_speed", self.fly_speed);
        s.set("cache_mb", crate::tiles::cache_cap_mb());
        s.set("ui_size", self.ui_size);
        s.set("bilingual", self.bilingual);
        s.set("favorites", self.favorites.join(","));
        for p in &self.planes {
            s.set(&format!("plane_{}", p.def.model), p.show);
        }
        s.text()
    }

    /// Applies saved settings (anything missing keeps its default).
    pub(crate) fn apply_settings(&mut self, s: &Store, ctx: &egui::Context) {
        if let Some(name) = s.get::<String>("palette") {
            if let Some(p) = Palette::all().find(|p| p.name() == name) {
                self.palette = p;
            }
        }
        macro_rules! take {
            ($field:expr, $key:expr) => {
                if let Some(v) = s.get($key) {
                    $field = v;
                }
            };
        }
        take!(self.show_labels, "labels");
        take!(self.label_bold, "label_bold");
        take!(self.show_compass, "compass");
        take!(self.transit_focus, "focus");
        take!(self.map_dim, "dim");
        take!(self.flat_stations, "flat_stations");
        take!(self.show_trains, "trains");
        take!(self.bilingual, "bilingual");
        if let Some(v) = s.get::<String>("favorites") {
            self.favorites = v.split(',').map(str::trim).filter(|x| !x.is_empty()).map(String::from).collect();
        }
        take!(self.route_color[0], "route_r");
        take!(self.route_color[1], "route_g");
        take!(self.route_color[2], "route_b");
        take!(self.route_alpha, "route_alpha");
        take!(self.route_width, "route_width");
        take!(self.route_at_altitude, "route_alt");
        take!(self.plane_size, "plane_size");
        take!(self.exaggeration, "exaggeration");
        if let Some(v) = s.get::<usize>("label_size") {
            self.label_size = v.min(crate::LABEL_SIZES.len() - 1);
        }
        if let Some(v) = s.get::<usize>("speed") {
            self.speed = v.min(SPEEDS.len() - 1);
        }
        if let Some(v) = s.get::<usize>("ui_size") {
            self.ui_size = v.min(crate::ui::UI_SIZES.len() - 1);
        }
        if let Some(v) = s.get::<u32>("anim_fps") {
            self.anim_fps = v.clamp(10, 120);
        }
        if let Some(v) = s.get::<usize>("train_speed") {
            self.train_speed = v.min(crate::TRAIN_SPEEDS.len() - 1);
        }
        if let Some(v) = s.get::<usize>("fly_speed") {
            self.fly_speed = v.min(crate::fly::FLY_SPEEDS.len() - 1);
        }
        if let Some(v) = s.get::<usize>("backend") {
            self.backend = v.min(crate::BACKENDS.len() - 1);
        }
        if let Some(v) = s.get::<u64>("cache_mb") {
            crate::tiles::set_cache_cap_mb(v);
        }
        if let Some(v) = s.get::<usize>("plane_quality") {
            self.plane_quality = v.min(1);
        }
        self.plane_size = self.plane_size.clamp(1.0, 20000.0);
        self.exaggeration = self.exaggeration.clamp(1.0, 150.0);
        self.map_dim = self.map_dim.clamp(0.0, 0.85);
        self.route_alpha = self.route_alpha.clamp(0.1, 1.0);
        self.route_width = self.route_width.clamp(1.0, 10.0);
        for p in &mut self.planes {
            if let Some(v) = s.get::<bool>(&format!("plane_{}", p.def.model)) {
                p.show = v;
            }
        }
        crate::ui::apply_scale(ctx, crate::ui::UI_SIZES[self.ui_size].1);
        if let Some(name) = s.get::<String>("font") {
            if let Some(i) = self.fonts.names().iter().position(|n| *n == name) {
                self.fonts.selected = i;
                self.fonts.apply(ctx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_round_trips_values() {
        let mut s = Store::default();
        s.set("a", 1.5f32);
        s.set("b", true);
        s.set("name", "Maps Light (Google-like)");
        let back = Store::parse(&s.text());
        assert_eq!(back.get::<f32>("a"), Some(1.5));
        assert_eq!(back.get::<bool>("b"), Some(true));
        assert_eq!(back.get::<String>("name").as_deref(), Some("Maps Light (Google-like)"));
        assert_eq!(back.get::<u8>("missing"), None);
        // junk is ignored
        let junk = Store::parse("nonsense\n=\nx=notanumber\n");
        assert_eq!(junk.get::<f32>("x"), None);
    }
}
