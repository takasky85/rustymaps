//! Map label fonts. Nothing is bundled or downloaded: the fonts already installed on the
//! computer are looked up by file name, a good one is picked automatically, and the user can
//! switch in the UI. Only the map labels use this font family; the UI keeps egui's own.
//!
//! A CJK-capable system font (Yu Gothic, Meiryo, Malgun, Noto CJK...) is appended as a fallback,
//! so Japanese / Chinese / Korean place names show up instead of empty boxes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

/// Name of the egui font family the map labels are drawn with.
pub const MAP_FAMILY: &str = "map-labels";
pub const MAP_BOLD_FAMILY: &str = "map-labels-bold";

/// A font we know how to find: display name, file name (compared in lower case), index inside a
/// .ttc collection.
struct Known {
    name: &'static str,
    file: &'static str,
    index: u32,
    /// The real bold face of the same family, when it ships as a separate file ("" = none).
    bold: &'static str,
    bold_index: u32,
}

const fn k(name: &'static str, file: &'static str, index: u32) -> Known {
    Known { name, file, index, bold: "", bold_index: 0 }
}

const fn kb(name: &'static str, file: &'static str, index: u32, bold: &'static str, bold_index: u32) -> Known {
    Known { name, file, index, bold, bold_index }
}

/// Offered in the picker when installed, in this order.
const KNOWN: &[Known] = &[
    // Windows
    kb("Segoe UI", "segoeui.ttf", 0, "segoeuib.ttf", 0),
    kb("Segoe UI Semibold", "seguisb.ttf", 0, "segoeuib.ttf", 0),
    k("Bahnschrift", "bahnschrift.ttf", 0),
    kb("Arial", "arial.ttf", 0, "arialbd.ttf", 0),
    kb("Arial Narrow", "arialn.ttf", 0, "arialnb.ttf", 0),
    kb("Calibri", "calibri.ttf", 0, "calibrib.ttf", 0),
    kb("Candara", "candara.ttf", 0, "candarab.ttf", 0),
    kb("Tahoma", "tahoma.ttf", 0, "tahomabd.ttf", 0),
    kb("Trebuchet MS", "trebuc.ttf", 0, "trebucbd.ttf", 0),
    kb("Verdana", "verdana.ttf", 0, "verdanab.ttf", 0),
    kb("Georgia", "georgia.ttf", 0, "georgiab.ttf", 0),
    kb("Yu Gothic", "yugothm.ttc", 0, "yugothb.ttc", 0),
    kb("Meiryo", "meiryo.ttc", 0, "meiryob.ttc", 0),
    // macOS
    k("Helvetica Neue", "helveticaneue.ttc", 0),
    kb("Helvetica", "helvetica.ttc", 0, "helvetica.ttc", 1),
    // Linux
    kb("Noto Sans", "notosans-regular.ttf", 0, "notosans-bold.ttf", 0),
    kb("Ubuntu", "ubuntu-r.ttf", 0, "ubuntu-b.ttf", 0),
    kb("DejaVu Sans", "dejavusans.ttf", 0, "dejavusans-bold.ttf", 0),
    kb("Liberation Sans", "liberationsans-regular.ttf", 0, "liberationsans-bold.ttf", 0),
];

/// What is used when the user has not chosen: the first of these that is installed.
const AUTO: &[&str] = &["Segoe UI", "Helvetica Neue", "Noto Sans", "Ubuntu", "Arial", "DejaVu Sans", "Liberation Sans"];

/// CJK fallbacks (file name, index), first installed one wins.
const FALLBACKS: &[(&str, u32)] = &[
    ("yugothm.ttc", 0),
    ("meiryo.ttc", 0),
    ("msgothic.ttc", 0),
    ("malgun.ttf", 0),
    ("msyh.ttc", 0),
    ("simsun.ttc", 0),
    ("notosanscjk-regular.ttc", 0),
    ("notosanscjkjp-regular.otf", 0),
    ("wqy-microhei.ttc", 0),
    ("arialuni.ttf", 0),
];

struct Installed {
    name: &'static str,
    path: PathBuf,
    index: u32,
    bold: Option<(PathBuf, u32)>,
}

pub struct Fonts {
    installed: Vec<Installed>,
    fallback: Option<(PathBuf, u32)>,
    /// 0 = egui's built-in font, otherwise 1 + index into `installed`.
    pub selected: usize,
}

impl Fonts {
    /// Look at the installed fonts and pick a good default.
    pub fn detect() -> Self {
        let mut files = HashMap::new();
        for dir in font_dirs() {
            scan(&dir, 0, &mut files);
        }
        Self::from_files(&files)
    }

    fn from_files(files: &HashMap<String, PathBuf>) -> Self {
        let installed: Vec<Installed> = KNOWN
            .iter()
            .filter_map(|f| {
                files.get(f.file).map(|p| Installed {
                    name: f.name,
                    path: p.clone(),
                    index: f.index,
                    bold: (!f.bold.is_empty()).then(|| files.get(f.bold).map(|b| (b.clone(), f.bold_index))).flatten(),
                })
            })
            .collect();
        let fallback = FALLBACKS.iter().find_map(|(f, i)| files.get(*f).map(|p| (p.clone(), *i)));
        let selected = AUTO
            .iter()
            .find_map(|a| installed.iter().position(|i| i.name == *a))
            .map(|p| p + 1)
            .unwrap_or(0);
        Fonts { installed, fallback, selected }
    }

    /// Names for the picker; index 0 is the built-in font.
    pub fn names(&self) -> Vec<&'static str> {
        std::iter::once("Built-in (Ubuntu)").chain(self.installed.iter().map(|i| i.name)).collect()
    }

    pub fn has_cjk(&self) -> bool {
        self.fallback.is_some()
    }

    /// The font family map labels should be drawn with.
    pub fn family() -> FontFamily {
        FontFamily::Name(MAP_FAMILY.into())
    }

    /// The same font in its real bold face.
    pub fn bold_family() -> FontFamily {
        FontFamily::Name(MAP_BOLD_FAMILY.into())
    }

    /// True when the chosen font has an installed bold face (otherwise bold is faked by overdrawing).
    pub fn has_real_bold(&self) -> bool {
        self.selected > 0 && self.installed.get(self.selected - 1).map_or(false, |f| f.bold.is_some())
    }

    /// (Re)build the label family for the current choice. Call at start-up and after a change.
    pub fn apply(&self, ctx: &egui::Context) {
        let mut defs = FontDefinitions::default();
        let mut family: Vec<String> = Vec::new();

        if self.selected > 0 {
            if let Some(f) = self.installed.get(self.selected - 1) {
                if let Some(data) = load(&f.path, f.index) {
                    defs.font_data.insert("map-main".to_owned(), Arc::new(data));
                    family.push("map-main".to_owned());
                }
            }
        }
        if let Some(base) = defs.families.get(&FontFamily::Proportional) {
            family.extend(base.iter().cloned());
        }
        if let Some((path, index)) = &self.fallback {
            if let Some(data) = load(path, *index) {
                defs.font_data.insert("map-cjk".to_owned(), Arc::new(data));
                family.push("map-cjk".to_owned());
            }
        }
        // The bold family: the real bold face first when there is one (else it just repeats the
        // regular one and the caller fakes the weight).
        let mut bold: Vec<String> = Vec::new();
        if self.selected > 0 {
            if let Some((path, index)) = self.installed.get(self.selected - 1).and_then(|f| f.bold.as_ref()) {
                if let Some(data) = load(path, *index) {
                    defs.font_data.insert("map-main-bold".to_owned(), Arc::new(data));
                    bold.push("map-main-bold".to_owned());
                }
            }
        }
        bold.extend(family.iter().cloned());
        defs.families.insert(Self::family(), family);
        defs.families.insert(Self::bold_family(), bold);
        ctx.set_fonts(defs);
    }
}

fn load(path: &Path, index: u32) -> Option<FontData> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let mut d = FontData::from_owned(bytes);
            d.index = index;
            Some(d)
        }
        Err(e) => {
            eprintln!("font {}: {e}", path.display());
            None
        }
    }
}

fn font_dirs() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    if let Some(w) = env("WINDIR").or_else(|| env("SystemRoot")) {
        v.push(w.join("Fonts"));
    }
    if let Some(l) = env("LOCALAPPDATA") {
        v.push(l.join("Microsoft").join("Windows").join("Fonts"));
    }
    if let Some(h) = env("HOME") {
        v.push(h.join(".fonts"));
        v.push(h.join(".local/share/fonts"));
        v.push(h.join("Library/Fonts"));
    }
    for d in [
        "/usr/share/fonts",
        "/usr/local/share/fonts",
        "/System/Library/Fonts",
        "/System/Library/Fonts/Supplemental",
        "/Library/Fonts",
    ] {
        v.push(PathBuf::from(d));
    }
    v.retain(|p| p.is_dir());
    v
}

/// Collect `lower-case file name -> path` for font files, a few folders deep.
fn scan(dir: &Path, depth: u32, out: &mut HashMap<String, PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth < 4 {
                scan(&p, depth + 1, out);
            }
        } else if let Some(n) = p.file_name().and_then(|n| n.to_str()) {
            let l = n.to_lowercase();
            if l.ends_with(".ttf") || l.ends_with(".ttc") || l.ends_with(".otf") {
                out.entry(l).or_insert(p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_choices_are_known_fonts() {
        for a in AUTO {
            assert!(KNOWN.iter().any(|f| f.name == *a), "{a}");
        }
        for (i, f) in KNOWN.iter().enumerate() {
            assert!(f.file == f.file.to_lowercase(), "{} must be lower case", f.file);
            assert!(!KNOWN[..i].iter().any(|g| g.name == f.name), "duplicate {}", f.name);
        }
    }

    #[test]
    fn picks_the_first_preferred_installed_font() {
        let mut files = HashMap::new();
        files.insert("arial.ttf".to_owned(), PathBuf::from("/x/arial.ttf"));
        files.insert("segoeui.ttf".to_owned(), PathBuf::from("/x/segoeui.ttf"));
        files.insert("meiryo.ttc".to_owned(), PathBuf::from("/x/meiryo.ttc"));
        let f = Fonts::from_files(&files);
        assert_eq!(f.names()[f.selected], "Segoe UI");
        assert!(f.has_cjk());
        assert_eq!(f.names()[0], "Built-in (Ubuntu)");
    }

    #[test]
    fn nothing_installed_means_built_in() {
        let f = Fonts::from_files(&HashMap::new());
        assert_eq!(f.selected, 0);
        assert!(!f.has_cjk());
        assert_eq!(f.names().len(), 1);
    }
}
