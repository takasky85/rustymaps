//! Map style: colors, zoom-dependent line widths, and label rules.
//!
//! Every drawable thing in a tile refers to a numeric style id. Fill styles are
//! drawn in ascending id order, then line styles in ascending id order, so the
//! numbering below is also the paint order. Colors and widths live in a small
//! table that is re-evaluated each frame from the zoom level, so the GPU never
//! needs per-feature data beyond the id.

use crate::mesh::Kind;

pub const BG: u32 = 0;
// fills
pub const COVER_ICE: u32 = 1;
pub const COVER_SAND: u32 = 2;
pub const COVER_FARM: u32 = 3;
pub const COVER_GRASS: u32 = 4;
pub const COVER_WETLAND: u32 = 5;
pub const COVER_WOOD: u32 = 6;
pub const LANDUSE_RES: u32 = 7;
pub const LANDUSE_OTHER: u32 = 8;
pub const PARK: u32 = 9;
pub const WATER: u32 = 10;
pub const BUILDING: u32 = 11;
// lines
pub const WATERWAY: u32 = 12;
pub const FERRY: u32 = 13;
pub const PATH: u32 = 14;
pub const RAIL: u32 = 15;
pub const ROAD_SERVICE: u32 = 16;
pub const ROAD_MINOR: u32 = 17;
pub const ROAD_TERTIARY: u32 = 18;
pub const ROAD_SECONDARY: u32 = 19;
pub const ROAD_PRIMARY: u32 = 20;
pub const ROAD_TRUNK: u32 = 21;
pub const ROAD_MOTORWAY: u32 = 22;
pub const BORDER_STATE: u32 = 23;
pub const BORDER_COUNTRY: u32 = 24;
// airfields (their colours are made from the palette's, see `Palette::colors`)
pub const AERO_FILL: u32 = 25;
pub const AERO_LINE: u32 = 26;

/// Number of style ids in use.
pub const COUNT: usize = 27;
/// Styles whose colour each palette lists by hand (the first ones).
const PALETTE_COLORS: usize = 25;
/// Size of the uniform table on the GPU (must match shader.wgsl).
pub const TABLE_SIZE: usize = 32;

/// Width in logical pixels, linearly interpolated in zoom; 0 (hidden) below `z0`.
fn lerp_w(zf: f64, z0: f64, w0: f64, z1: f64, w1: f64) -> f32 {
    if zf < z0 {
        return 0.0;
    }
    let t = ((zf - z0) / (z1 - z0)).clamp(0.0, 1.0);
    (w0 + (w1 - w0) * t) as f32
}

/// Selectable map looks: colour recreations of well-known OpenStreetMap styles. They all draw the same
/// OpenFreeMap data, so layers and label content are the same; only colours differ (and dark styles
/// switch to light labels).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Palette(pub usize);

struct PaletteDef {
    name: &'static str,
    dark: bool,
    /// 0xRRGGBB per style id (see the constants above).
    colors: [u32; PALETTE_COLORS],
}

const PALETTES: &[PaletteDef] = &[
    PaletteDef {
        name: "OSM Standard",
        dark: false,
        colors: [
            0xf2efe9, 0xffffff, 0xf5e9c6, 0xeef0d5, 0xcdebb0, 0xb5d6c1,
            0xadd19e, 0xe0dfdf, 0xe7dee8, 0xc8facc, 0xaad3df, 0xd9d0c9,
            0xaad3df, 0x6699cc, 0xbea096, 0x969696, 0xffffff, 0xffffff,
            0xffffff, 0xf7fabf, 0xfcd6a4, 0xf9b29c, 0xe892a2, 0xac8cac,
            0x8c648c,
        ],
    },
    PaletteDef {
        name: "OSM Bright",
        dark: false,
        colors: [
            0xf8f4e8, 0xffffff, 0xfce8a0, 0xf3f5be, 0xb2e88c, 0x96dab4,
            0x80d06e, 0xf0e8da, 0xebe4f4, 0x96e482, 0x60bef0, 0xe4d2c4,
            0x60bef0, 0x2878dc, 0xdc9682, 0x8c8c96, 0xffffff, 0xffffff,
            0xffffff, 0xfff478, 0xffc850, 0xff9664, 0xff648c, 0xaa6ebe,
            0x783c96,
        ],
    },
    PaletteDef {
        name: "OpenFreeMap Liberty",
        dark: false,
        colors: [
            0xf8f4f0, 0xffffff, 0xf6ebc8, 0xeef0d6, 0xd8e8c8, 0xcfe5d4,
            0xc8dfb4, 0xede8e2, 0xe9e4ee, 0xcfe8c4, 0xa0c8f0, 0xe0d8d0,
            0xa0c8f0, 0x6f9bd6, 0xd0b8a8, 0xb0b0b8, 0xffffff, 0xffffff,
            0xffffff, 0xfff2c0, 0xffd8a0, 0xffbf8c, 0xff9f6e, 0xb8a0b8,
            0x9a7fa0,
        ],
    },
    PaletteDef {
        name: "Positron (light)",
        dark: false,
        colors: [
            0xf5f5f2, 0xffffff, 0xf4f0e4, 0xf0f2ea, 0xe6eee0, 0xe0ece6,
            0xdce8d4, 0xeeeeea, 0xeaeaee, 0xe2eddc, 0xd0dee8, 0xe8e6e2,
            0xd0dee8, 0xaabed2, 0xe1dad4, 0xc8c8cc, 0xffffff, 0xffffff,
            0xffffff, 0xfcfcf6, 0xfaf4e8, 0xf8ece0, 0xf6e2d6, 0xc4b8cc,
            0xa090aa,
        ],
    },
    PaletteDef {
        name: "Dark Matter",
        dark: true,
        colors: [
            0x1f2125, 0x2c2f33, 0x24262a, 0x22252a, 0x232b27, 0x22302d,
            0x212c27, 0x25272b, 0x262830, 0x233028, 0x0f1217, 0x2d3036,
            0x0f1217, 0x3a5f8a, 0x3a3d44, 0x4a4d55, 0x33363d, 0x3a3d45,
            0x41444c, 0x4b4e57, 0x565963, 0x626571, 0x6f7380, 0x5a5560,
            0x7c7587,
        ],
    },
    PaletteDef {
        name: "Voyager",
        dark: false,
        colors: [
            0xf3f1ec, 0xffffff, 0xefe9d8, 0xecebdd, 0xd6e6c8, 0xcde3d6,
            0xc8dcb4, 0xe8e4dd, 0xe8e2ea, 0xcfe6c2, 0xa6cde4, 0xe0dbd3,
            0xa6cde4, 0x7aa6d0, 0xd8c4b8, 0xb8b8b8, 0xffffff, 0xffffff,
            0xffffff, 0xfff0b8, 0xfdd9a0, 0xf9c08c, 0xf4a07a, 0xc0a8c0,
            0x9d86a3,
        ],
    },
    PaletteDef {
        name: "OSM Humanitarian",
        dark: false,
        colors: [
            0xf2eee6, 0xffffff, 0xf2e6b8, 0xebe9cc, 0xd4e6b4, 0xbcd9c4,
            0xb8d29a, 0xe6e0d6, 0xe8dfe4, 0xc8e6b4, 0xb5d0d0, 0xd9c6b8,
            0xb5d0d0, 0x7aa0c4, 0xc08870, 0x808080, 0xffffff, 0xffffff,
            0xfff8d8, 0xfbea9a, 0xf8d070, 0xf2a060, 0xe87860, 0xb080a0,
            0x905070,
        ],
    },
    PaletteDef {
        name: "OpenTopoMap",
        dark: false,
        colors: [
            0xf0edd8, 0xf4fbff, 0xf2e4b0, 0xeaeac8, 0xd2e5a8, 0xb8dcc0,
            0xa8cf8a, 0xe4dccc, 0xe0d8d0, 0xbfe0a0, 0xb0cde8, 0xd0c0b0,
            0x80a8d8, 0x4a78b0, 0x8a5a3a, 0x707070, 0xf5f0e0, 0xffffff,
            0xfff2a0, 0xffe070, 0xffb050, 0xff8040, 0xf06040, 0xa070a0,
            0x704070,
        ],
    },
    PaletteDef {
        name: "CyclOSM",
        dark: false,
        colors: [
            0xf3f0e6, 0xffffff, 0xf6ecc8, 0xecefd2, 0xd0e8b0, 0xbcdcc8,
            0xb0d49a, 0xe6e0dc, 0xe8e0ec, 0xc4eac4, 0xa8d0e8, 0xdcd0c8,
            0xa8d0e8, 0x5a88c8, 0x2848e0, 0x808080, 0xffffff, 0xffffff,
            0xfff0a0, 0xffd870, 0xffb060, 0xff9060, 0xe87070, 0xb098b0,
            0x8a6e8e,
        ],
    },
    PaletteDef {
        name: "OSM Transport",
        dark: false,
        colors: [
            0xefefe8, 0xffffff, 0xece8d0, 0xe8e8d8, 0xd8e8c8, 0xd0e0d0,
            0xc8dcb8, 0xe4e4e0, 0xe4e0e8, 0xc8e4c0, 0xb4d4ec, 0xd8d4d0,
            0xb4d4ec, 0x6090c0, 0xc8b8b0, 0x6a6a78, 0xffffff, 0xffffff,
            0xffffff, 0xffe8a0, 0xffc880, 0xffa860, 0xff8860, 0xb098b0,
            0x8a6e8e,
        ],
    },
    PaletteDef {
        name: "Stamen Terrain",
        dark: false,
        colors: [
            0xe8ecd4, 0xffffff, 0xf0e6b8, 0xe4ecc4, 0xd4e4b4, 0xbcdcc0,
            0xa8cc8c, 0xe4e2d0, 0xe0dccc, 0xc0dca0, 0x9cc4e4, 0xd4c8b8,
            0x9cc4e4, 0x5a88c0, 0xa07050, 0x808078, 0xf8f4e8, 0xf8f4e0,
            0xf4e8b0, 0xf0d890, 0xe8b868, 0xe09858, 0xd87850, 0xa07898,
            0x785880,
        ],
    },
    PaletteDef {
        name: "Stamen Toner",
        dark: false,
        colors: [
            0xffffff, 0xffffff, 0xffffff, 0xffffff, 0xf0f0f0, 0xe0e0e0,
            0xe6e6e6, 0xf6f6f6, 0xececec, 0xe8e8e8, 0x404040, 0xc8c8c8,
            0x404040, 0x606060, 0x909090, 0x707070, 0x9a9a9a, 0x000000,
            0x000000, 0x000000, 0x000000, 0x000000, 0x000000, 0x808080,
            0x000000,
        ],
    },
    PaletteDef {
        name: "Monochrome",
        dark: false,
        colors: [
            0xe8e8e8, 0xffffff, 0xe4e4e4, 0xe6e6e6, 0xdedede, 0xd8d8d8,
            0xd4d4d4, 0xe0e0e0, 0xdcdcdc, 0xdadada, 0xbcbcbc, 0xd0d0d0,
            0xbcbcbc, 0x989898, 0xb0b0b0, 0xa0a0a0, 0xf4f4f4, 0xffffff,
            0xffffff, 0xf8f8f8, 0xf0f0f0, 0xe6e6e6, 0xd8d8d8, 0xa0a0a0,
            0x707070,
        ],
    },
    PaletteDef {
        name: "Vintage",
        dark: false,
        colors: [
            0xefe3c8, 0xf8f2e0, 0xead8a8, 0xe6d9b0, 0xd8d0a0, 0xc8cca8,
            0xc0c490, 0xe4d6b8, 0xdcccb0, 0xccd0a0, 0xb8c8c0, 0xd2bf9c,
            0xb8c8c0, 0x7a98a0, 0xa88860, 0x8a7a60, 0xf6ecd4, 0xf8f0d8,
            0xf6e8c0, 0xf0d8a0, 0xe8c080, 0xdca868, 0xc88858, 0xa07860,
            0x805840,
        ],
    },
    PaletteDef {
        name: "Midnight",
        dark: true,
        colors: [
            0x14171f, 0x262c3a, 0x1a1d28, 0x171b24, 0x18212a, 0x172430,
            0x162028, 0x1a1d27, 0x1c1f2a, 0x182430, 0x0a1230, 0x262a38,
            0x0a1230, 0x3a60b0, 0x3a4054, 0x50587a, 0x2a2f40, 0x303650,
            0x3a4262, 0x4a5478, 0x5c6890, 0x7080a8, 0x90a0d0, 0x5a5078,
            0x8a80b0,
        ],
    },
    PaletteDef {
        name: "Blueprint",
        dark: true,
        colors: [
            0x17406e, 0x1c4a80, 0x1b4676, 0x194372, 0x1b4878, 0x1c4a7c,
            0x1a4474, 0x1a4676, 0x1c4878, 0x1d4c80, 0x0f2f58, 0x2a5a96,
            0x0f2f58, 0x7ab0e8, 0x4a7cb8, 0x6a98d0, 0x3b6aa8, 0x5a88c4,
            0x6c9ad2, 0x7eaae0, 0x94bcec, 0xaaccf4, 0xc4defc, 0x6a98d0,
            0x9cc2f0,
        ],
    },
    // Inspired by the look of Google Maps: pale warm-grey land, soft green parks, light blue water,
    // white streets with a grey edge and warm yellow-orange highways.
    PaletteDef {
        name: "Google-like Light",
        dark: false,
        colors: [
            0xf3f2ee, 0xffffff, 0xf2ecd9, 0xecf0dd, 0xdcecd0, 0xd0e6d3,
            0xc9e3bd, 0xeeede8, 0xe8e7e2, 0xc6e7c0, 0xaadaff, 0xe4e2dc,
            0xaadaff, 0x6aa7e8, 0xd9d6cf, 0xbcc0c6, 0xffffff, 0xffffff,
            0xffffff, 0xfff6dc, 0xfde9b2, 0xfbd67e, 0xf9c14a, 0xcfc9db,
            0xa89fba,
        ],
    },
    // The dark "night" look often used with Google Maps.
    PaletteDef {
        name: "Google-like Night",
        dark: true,
        colors: [
            0x242f3e, 0x3a4556, 0x2b3646, 0x28344a, 0x263c3f, 0x253a3d,
            0x233a3a, 0x2a3547, 0x273244, 0x263c3f, 0x17263c, 0x2f3b4d,
            0x17263c, 0x3b5e8c, 0x38414e, 0x3b4658, 0x38414e, 0x38414e,
            0x3a4352, 0x41495a, 0x5a5648, 0x6a5f4a, 0x746855, 0x3d4b60,
            0x4b6878,
        ],
    },
];

impl Palette {
    pub const STANDARD: Palette = Palette(0);
    #[allow(dead_code)]
    pub const BRIGHT: Palette = Palette(1);

    pub fn all() -> impl Iterator<Item = Palette> {
        (0..PALETTES.len()).map(Palette)
    }

    pub fn name(self) -> &'static str {
        PALETTES[self.0].name
    }

    /// Four representative colours (land, water, park, main road) as 0xRRGGBB, for little previews.
    pub fn swatch(self) -> [u32; 4] {
        let c = &PALETTES[self.0].colors;
        [c[BG as usize], c[WATER as usize], c[PARK as usize], c[ROAD_PRIMARY as usize]]
    }

    /// Dark map: labels switch to light text on a dark halo.
    pub fn dark(self) -> bool {
        PALETTES[self.0].dark
    }

    /// RGB per style id (see the constants above).
    fn colors(self) -> [[u8; 3]; COUNT] {
        let mut out = [[0u8; 3]; COUNT];
        for (o, c) in out.iter_mut().zip(PALETTES[self.0].colors) {
            *o = [(c >> 16) as u8, (c >> 8) as u8, c as u8];
        }
        // airfields: ground a little greyer than other land use, runways a middle grey-white
        let mix = |a: [u8; 3], b: [u8; 3], t: f32| [0, 1, 2].map(|k| (a[k] as f32 + (b[k] as f32 - a[k] as f32) * t).round() as u8);
        out[AERO_FILL as usize] = mix(out[LANDUSE_OTHER as usize], out[RAIL as usize], 0.25);
        out[AERO_LINE as usize] = mix(out[ROAD_SERVICE as usize], out[RAIL as usize], 0.45);
        out
    }
}

/// Style table for the shader: per id `[r, g, b, a, width_px, casing_px, 0, 0]`.
pub fn table(zf: f64, ppp: f32, palette: Palette, detail: &crate::detail::MapDetail) -> [[f32; 8]; TABLE_SIZE] {
    let mut t = [[0.0f32; 8]; TABLE_SIZE];
    let cols = palette.colors();
    let rgb = |id: u32| {
        let c = cols[id as usize];
        [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0]
    };

    for id in BG..=BUILDING {
        let c = rgb(id);
        // Flat building footprints fade in at city scale.
        let a = if id == BUILDING { ((zf - 13.0).clamp(0.0, 1.0)) as f32 } else { 1.0 };
        t[id as usize] = [c[0], c[1], c[2], a, 0.0, 0.0, 0.0, 0.0];
    }

    let mut line = |id: u32, w: f32| {
        let c = rgb(id);
        // A thin darker casing makes wide roads readable on light ground.
        let casing = if w >= 3.0 { 1.0 } else { 0.0 };
        t[id as usize] = [c[0], c[1], c[2], 1.0, w * ppp, casing * ppp, 0.0, 0.0];
    };
    line(WATERWAY, lerp_w(zf, 7.0, 0.6, 16.0, 3.5));
    line(FERRY, lerp_w(zf, 8.0, 0.8, 16.0, 1.5));
    line(PATH, lerp_w(zf, 15.0, 0.8, 17.0, 1.8));
    line(RAIL, lerp_w(zf, 9.0, 0.8, 16.0, 2.0));
    line(ROAD_SERVICE, lerp_w(zf, 15.0, 1.0, 17.0, 5.0));
    line(ROAD_MINOR, lerp_w(zf, 13.5, 0.8, 17.0, 7.0));
    line(ROAD_TERTIARY, lerp_w(zf, 12.0, 0.8, 17.0, 8.5));
    line(ROAD_SECONDARY, lerp_w(zf, 10.0, 0.9, 17.0, 10.0));
    line(ROAD_PRIMARY, lerp_w(zf, 7.0, 1.0, 17.0, 11.0));
    line(ROAD_TRUNK, lerp_w(zf, 6.0, 1.1, 17.0, 11.5));
    line(ROAD_MOTORWAY, lerp_w(zf, 4.5, 1.2, 17.0, 12.0));
    line(BORDER_STATE, lerp_w(zf, 3.5, 0.7, 8.0, 1.3));
    line(BORDER_COUNTRY, lerp_w(zf, 0.0, 0.9, 8.0, 1.8));
    // runways and taxiways
    line(AERO_LINE, lerp_w(zf, 9.0, 1.2, 17.0, 9.0));
    {
        let c = rgb(AERO_FILL);
        t[AERO_FILL as usize] = [c[0], c[1], c[2], ((zf - 10.0) / 2.0).clamp(0.0, 1.0) as f32, 0.0, 0.0, 0.0, 0.0];
    }
    // Small features thin out towards the horizon of a tilted view (see vs_line).
    for id in [PATH, ROAD_SERVICE, ROAD_MINOR, ROAD_TERTIARY, ROAD_SECONDARY, RAIL, WATERWAY, FERRY] {
        t[id as usize][6] = 1.0;
    }
    // what the user has switched off is drawn with no colour and no width
    for id in 1..COUNT as u32 {
        if !detail.style_on(id) {
            let e = &mut t[id as usize];
            e[3] = 0.0;
            e[4] = 0.0;
            e[5] = 0.0;
        }
    }
    t
}

/// How a label should look at the current zoom, or `None` to hide it.
pub struct LabelRule {
    pub size: f32,
    pub color: [u8; 3],
    pub halo: [u8; 3],
    /// Lower is placed first (wins collisions).
    pub priority: f32,
    pub upper: bool,
}

pub fn label_rule(kind: Kind, rank: u32, zf: f64, dark: bool) -> Option<LabelRule> {
    let mut rule = label_rule_light(kind, rank, zf)?;
    if dark {
        let lift = |c: [u8; 3]| [255 - c[0] / 5, 255 - c[1] / 5, 255 - c[2] / 5];
        rule.color = lift(rule.color);
        rule.halo = [16, 18, 24];
    }
    Some(rule)
}

fn label_rule_light(kind: Kind, rank: u32, zf: f64) -> Option<LabelRule> {
    let r = rank.min(100) as f32 * 0.05;
    let white = [255, 255, 255];
    let water_halo = [160, 198, 236];
    match kind {
        Kind::Continent => (zf < 3.0).then_some(LabelRule {
            size: 17.0,
            color: [110, 105, 120],
            halo: white,
            priority: 0.0 + r,
            upper: true,
        }),
        Kind::Ocean => (zf < 6.0).then_some(LabelRule {
            size: 15.0,
            color: [70, 100, 150],
            halo: water_halo,
            priority: 1.0 + r,
            upper: true,
        }),
        Kind::Sea => (2.0..9.0).contains(&zf).then_some(LabelRule {
            size: 12.5,
            color: [70, 100, 150],
            halo: water_halo,
            priority: 4.0 + r,
            upper: false,
        }),
        Kind::Country => (0.5..8.5).contains(&zf).then_some(LabelRule {
            size: (13.0 + zf as f32 * 0.9).min(19.0),
            color: [70, 60, 90],
            halo: white,
            priority: 2.0 + r,
            upper: true,
        }),
        Kind::State => (4.0..9.5).contains(&zf).then_some(LabelRule {
            size: 11.5,
            color: [130, 110, 140],
            halo: white,
            priority: 6.0 + r,
            upper: true,
        }),
        Kind::City => (zf >= 2.4).then_some(LabelRule {
            size: if rank <= 3 { 14.0 } else { 12.5 },
            color: [30, 30, 35],
            halo: white,
            priority: 10.0 + r,
            upper: false,
        }),
        Kind::Island => (zf >= 5.5).then_some(LabelRule {
            size: 11.0,
            color: [90, 110, 90],
            halo: white,
            priority: 25.0 + r,
            upper: false,
        }),
        Kind::Town => (zf >= 8.0).then_some(LabelRule {
            size: 11.5,
            color: [45, 45, 50],
            halo: white,
            priority: 20.0 + r,
            upper: false,
        }),
        Kind::Village => (zf >= 10.5).then_some(LabelRule {
            size: 10.5,
            color: [70, 70, 75],
            halo: white,
            priority: 30.0 + r,
            upper: false,
        }),
        Kind::Suburb => (zf >= 12.0).then_some(LabelRule {
            size: 10.0,
            color: [110, 110, 115],
            halo: white,
            priority: 40.0 + r,
            upper: false,
        }),
        Kind::Poi => {
            // rank: 1 is the most important; more of them appear as you zoom in
            let limit = if zf < 15.0 {
                return None;
            } else if zf < 16.0 {
                8
            } else if zf < 17.0 {
                30
            } else if zf < 17.8 {
                100
            } else {
                100_000
            };
            (rank <= limit).then_some(LabelRule { size: 10.0, color: [55, 55, 65], halo: white, priority: 45.0 + rank.min(100) as f32 * 0.04, upper: false })
        }
        Kind::HouseNumber => (zf >= 17.6).then_some(LabelRule { size: 9.0, color: [120, 105, 95], halo: white, priority: 70.0, upper: false }),
        Kind::Peak => (zf >= 9.0).then_some(LabelRule { size: 10.5, color: [120, 82, 52], halo: white, priority: 24.0 + r, upper: false }),
        Kind::Airport => (7.5..16.5).contains(&zf).then_some(LabelRule { size: 11.0, color: [60, 62, 120], halo: white, priority: 12.0 + r, upper: false }),
        // Street names: rank is the road class (0 motorway .. 6 service); smaller roads need more zoom.
        Kind::Street => {
            let from = [13.5, 13.5, 13.5, 14.5, 15.0, 15.5, 16.5][rank.min(6) as usize];
            (zf >= from).then_some(LabelRule {
                size: 11.0,
                color: [75, 75, 85],
                halo: white,
                priority: 50.0 + rank.min(6) as f32,
                upper: false,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detail::MapDetail;

    #[test]
    fn table_has_every_style_and_hides_by_zoom() {
        let far = table(1.0, 1.0, Palette::STANDARD, &MapDetail::default());
        let near = table(14.0, 1.0, Palette::STANDARD, &MapDetail::default());
        assert!(far[ROAD_MINOR as usize][4] == 0.0, "minor roads hidden at globe scale");
        assert!(near[ROAD_MINOR as usize][4] > 1.0, "minor roads visible at city scale");
        assert!(far[BORDER_COUNTRY as usize][4] > 0.0, "country borders always visible");
        assert!(far[BUILDING as usize][3] == 0.0 && near[BUILDING as usize][3] == 1.0);
        for id in 0..COUNT {
            assert!(near[id][3] >= 0.0);
        }
        // a switched-off style has no colour and no width
        let mut off = MapDetail::default();
        off.set(crate::detail::ROADS_LOCAL, false);
        off.set(crate::detail::BUILDINGS, false);
        let hidden = table(14.0, 1.0, Palette::STANDARD, &off);
        assert!(hidden[ROAD_MINOR as usize][3] == 0.0 && hidden[ROAD_MINOR as usize][4] == 0.0 && hidden[BUILDING as usize][3] == 0.0);
        assert!(hidden[ROAD_PRIMARY as usize][4] > 0.0 && hidden[BG as usize][3] == 1.0);
        // every palette defines every style, and they really differ
        let std_water = table(5.0, 1.0, Palette::STANDARD, &MapDetail::default())[WATER as usize];
        let bright_water = table(5.0, 1.0, Palette::BRIGHT, &MapDetail::default())[WATER as usize];
        assert!(std_water[..3] != bright_water[..3]);
        // pixel-ratio scaling applies to widths
        assert_eq!(table(14.0, 2.0, Palette::STANDARD, &MapDetail::default())[ROAD_MINOR as usize][4], 2.0 * near[ROAD_MINOR as usize][4]);
    }

    #[test]
    fn label_rules_follow_zoom() {
        assert!(label_rule(Kind::Country, 1, 2.0, false).is_some());
        assert!(label_rule(Kind::Country, 1, 12.0, false).is_none());
        assert!(label_rule(Kind::Village, 50, 5.0, false).is_none());
        assert!(label_rule(Kind::Village, 50, 12.0, false).is_some());
        let big = label_rule(Kind::City, 1, 5.0, false).unwrap();
        let small = label_rule(Kind::City, 20, 5.0, false).unwrap();
        assert!(big.priority < small.priority);
    }

    #[test]
    fn many_palettes_all_distinct_and_dark_ones_flip_labels() {
        assert!(Palette::all().count() >= 15);
        let mut seen = std::collections::HashSet::new();
        for p in Palette::all() {
            assert!(seen.insert(p.colors()), "{} duplicates another style", p.name());
            let t = table(12.0, 1.0, p, &MapDetail::default());
            assert!(t[WATER as usize][3] == 1.0 && t[ROAD_PRIMARY as usize][4] > 0.0);
        }
        let dark = Palette::all().find(|p| p.dark()).expect("a dark style exists");
        let light = Palette::STANDARD;
        let a = label_rule(Kind::City, 1, 5.0, true).unwrap();
        let b = label_rule(Kind::City, 1, 5.0, false).unwrap();
        assert!(a.color[0] > 200 && b.color[0] < 80, "{:?} {:?}", a.color, b.color);
        assert!(dark.dark() && !light.dark());
    }
}
