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

/// Number of style ids in use.
pub const COUNT: usize = 25;
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

/// Style table for the shader: per id `[r, g, b, a, width_px, casing_px, 0, 0]`.
pub fn table(zf: f64, ppp: f32) -> [[f32; 8]; TABLE_SIZE] {
    let mut t = [[0.0f32; 8]; TABLE_SIZE];

    let mut fill = |id: u32, rgb: [u8; 3], a: f32| {
        t[id as usize] = [
            rgb[0] as f32 / 255.0,
            rgb[1] as f32 / 255.0,
            rgb[2] as f32 / 255.0,
            a,
            0.0,
            0.0,
            0.0,
            0.0,
        ];
    };
    fill(BG, [236, 233, 224], 1.0);
    fill(COVER_ICE, [250, 252, 255], 1.0);
    fill(COVER_SAND, [240, 232, 200], 1.0);
    fill(COVER_FARM, [233, 236, 214], 1.0);
    fill(COVER_GRASS, [214, 230, 190], 1.0);
    fill(COVER_WETLAND, [205, 226, 205], 1.0);
    fill(COVER_WOOD, [190, 218, 170], 1.0);
    fill(LANDUSE_RES, [231, 227, 217], 1.0);
    fill(LANDUSE_OTHER, [224, 224, 214], 1.0);
    fill(PARK, [200, 228, 175], 1.0);
    fill(WATER, [150, 190, 232], 1.0);
    // Flat building footprints fade in at city scale.
    fill(BUILDING, [214, 206, 196], ((zf - 13.0).clamp(0.0, 1.0)) as f32);

    let mut line = |id: u32, rgb: [u8; 3], w: f32| {
        // A thin darker casing makes wide roads readable on light ground.
        let casing = if w >= 3.0 { 1.0 } else { 0.0 };
        t[id as usize] = [
            rgb[0] as f32 / 255.0,
            rgb[1] as f32 / 255.0,
            rgb[2] as f32 / 255.0,
            1.0,
            w * ppp,
            casing * ppp,
            0.0,
            0.0,
        ];
    };
    line(WATERWAY, [150, 190, 232], lerp_w(zf, 7.0, 0.6, 16.0, 3.5));
    line(FERRY, [110, 140, 190], lerp_w(zf, 8.0, 0.8, 16.0, 1.5));
    line(PATH, [205, 195, 185], lerp_w(zf, 14.0, 0.8, 17.0, 1.8));
    line(RAIL, [140, 140, 145], lerp_w(zf, 9.0, 0.8, 16.0, 2.0));
    line(ROAD_SERVICE, [255, 255, 255], lerp_w(zf, 14.0, 1.0, 17.0, 5.0));
    line(ROAD_MINOR, [255, 255, 255], lerp_w(zf, 12.5, 0.8, 17.0, 7.0));
    line(ROAD_TERTIARY, [255, 255, 255], lerp_w(zf, 11.0, 0.8, 17.0, 8.5));
    line(ROAD_SECONDARY, [255, 240, 160], lerp_w(zf, 9.0, 0.9, 17.0, 10.0));
    line(ROAD_PRIMARY, [253, 222, 140], lerp_w(zf, 7.0, 1.0, 17.0, 11.0));
    line(ROAD_TRUNK, [250, 200, 120], lerp_w(zf, 6.0, 1.1, 17.0, 11.5));
    line(ROAD_MOTORWAY, [244, 170, 110], lerp_w(zf, 4.5, 1.2, 17.0, 12.0));
    line(BORDER_STATE, [160, 140, 170], lerp_w(zf, 3.5, 0.7, 8.0, 1.3));
    line(BORDER_COUNTRY, [140, 110, 150], lerp_w(zf, 0.0, 0.9, 8.0, 1.8));
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

pub fn label_rule(kind: Kind, rank: u32, zf: f64) -> Option<LabelRule> {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_every_style_and_hides_by_zoom() {
        let far = table(1.0, 1.0);
        let near = table(14.0, 1.0);
        assert!(far[ROAD_MINOR as usize][4] == 0.0, "minor roads hidden at globe scale");
        assert!(near[ROAD_MINOR as usize][4] > 1.0, "minor roads visible at city scale");
        assert!(far[BORDER_COUNTRY as usize][4] > 0.0, "country borders always visible");
        assert!(far[BUILDING as usize][3] == 0.0 && near[BUILDING as usize][3] == 1.0);
        for id in 0..COUNT {
            assert!(near[id][3] >= 0.0);
        }
        // pixel-ratio scaling applies to widths
        assert_eq!(table(14.0, 2.0)[ROAD_MINOR as usize][4], 2.0 * near[ROAD_MINOR as usize][4]);
    }

    #[test]
    fn label_rules_follow_zoom() {
        assert!(label_rule(Kind::Country, 1, 2.0).is_some());
        assert!(label_rule(Kind::Country, 1, 12.0).is_none());
        assert!(label_rule(Kind::Village, 50, 5.0).is_none());
        assert!(label_rule(Kind::Village, 50, 12.0).is_some());
        let big = label_rule(Kind::City, 1, 5.0).unwrap();
        let small = label_rule(Kind::City, 20, 5.0).unwrap();
        assert!(big.priority < small.priority);
    }
}
