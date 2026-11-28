//! Which parts of the map are drawn. Every switch here hides something that is in the map tiles:
//! a kind of area, a class of road, a sort of label. Nothing is downloaded or rebuilt when one is
//! flipped: hidden styles are simply drawn with no colour (see `style::table`) and hidden labels are
//! skipped when labels are placed, so it takes effect on the very next frame.

use crate::style as st;

#[derive(Clone, Copy)]
pub struct Item {
    /// Name in the settings file.
    pub key: &'static str,
    /// Text in the panel.
    pub name: &'static str,
    /// The heading it is listed under.
    pub group: &'static str,
    /// Drawn unless the user switches it off.
    pub default_on: bool,
}

macro_rules! items {
    ($( $idx:ident = ($key:expr, $name:expr, $group:expr, $on:expr) ),+ $(,)?) => {
        #[allow(non_camel_case_types, dead_code, clippy::upper_case_acronyms)]
        #[repr(usize)]
        enum Idx { $( $idx ),+ }
        $( #[allow(dead_code)] pub const $idx: usize = Idx::$idx as usize; )+
        pub const ITEMS: &[Item] = &[ $( Item { key: $key, name: $name, group: $group, default_on: $on } ),+ ];
    };
}

items! {
    WATER = ("water", "Seas, lakes and rivers", "Land and water", true),
    WATERWAYS = ("waterways", "Streams and canals", "Land and water", true),
    PARKS = ("parks", "Parks", "Land and water", true),
    WOODS = ("woods", "Forests", "Land and water", true),
    GRASS = ("grass", "Grass and farmland", "Land and water", true),
    WETLAND = ("wetland", "Wetland", "Land and water", true),
    SAND = ("sand", "Sand and beaches", "Land and water", true),
    ICE = ("ice", "Ice and glaciers", "Land and water", true),
    LANDUSE = ("landuse", "Land use (housing, industry...)", "Land and water", true),
    BUILDINGS = ("buildings", "Buildings", "Built up", true),
    AIRFIELDS = ("airfields", "Runways and aprons", "Built up", true),
    ROADS_MAJOR = ("roads_major", "Motorways and trunk roads", "Roads and rail", true),
    ROADS_MAIN = ("roads_main", "Main roads", "Roads and rail", true),
    ROADS_LOCAL = ("roads_local", "Local streets", "Roads and rail", true),
    ROADS_SERVICE = ("roads_service", "Service roads and alleys", "Roads and rail", true),
    PATHS = ("paths", "Footpaths and tracks", "Roads and rail", true),
    RAIL = ("rail", "Railways", "Roads and rail", true),
    FERRIES = ("ferries", "Ferry routes", "Roads and rail", true),
    BORDER_COUNTRY = ("border_country", "Country borders", "Borders", true),
    BORDER_STATE = ("border_state", "State borders", "Borders", true),
    L_REGIONS = ("l_regions", "Countries, states, oceans", "Names", true),
    L_PLACES = ("l_places", "Cities, towns, villages", "Names", true),
    L_AREAS = ("l_areas", "Neighbourhoods and islands", "Names", true),
    L_STREETS = ("l_streets", "Street names", "Names", true),
    L_HOUSES = ("l_houses", "House numbers", "Names", true),
    L_PEAKS = ("l_peaks", "Mountain peaks", "Names", true),
    L_AIRPORTS = ("l_airports", "Airports", "Names", true),
    POI_FOOD = ("poi_food", "Food and drink", "Places of interest", true),
    POI_SHOPS = ("poi_shops", "Shops", "Places of interest", true),
    POI_TRANSIT = ("poi_transit", "Stops, stations, parking, fuel", "Places of interest", true),
    POI_HEALTH = ("poi_health", "Health", "Places of interest", true),
    POI_OUTDOORS = ("poi_outdoors", "Parks, sport, playgrounds", "Places of interest", true),
    POI_CULTURE = ("poi_culture", "Museums, sights, theatres", "Places of interest", true),
    POI_SERVICES = ("poi_services", "Services, schools, offices, other", "Places of interest", true),
}

/// The state of every switch.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MapDetail {
    on: Vec<bool>,
}

impl Default for MapDetail {
    fn default() -> Self {
        MapDetail { on: ITEMS.iter().map(|i| i.default_on).collect() }
    }
}

impl MapDetail {
    pub fn get(&self, i: usize) -> bool {
        self.on.get(i).copied().unwrap_or(true)
    }

    pub fn set(&mut self, i: usize, v: bool) {
        if let Some(x) = self.on.get_mut(i) {
            *x = v;
        }
    }

    pub fn flag_mut(&mut self, i: usize) -> &mut bool {
        &mut self.on[i]
    }

    pub fn set_all(&mut self, group: Option<&str>, v: bool) {
        for (i, it) in ITEMS.iter().enumerate() {
            if group.map_or(true, |g| g == it.group) {
                self.on[i] = v;
            }
        }
    }

    /// False for a style (see `style`) whose switch is off.
    pub fn style_on(&self, id: u32) -> bool {
        let item = match id {
            st::WATER => WATER,
            st::WATERWAY => WATERWAYS,
            st::PARK => PARKS,
            st::COVER_WOOD => WOODS,
            st::COVER_GRASS | st::COVER_FARM => GRASS,
            st::COVER_WETLAND => WETLAND,
            st::COVER_SAND => SAND,
            st::COVER_ICE => ICE,
            st::LANDUSE_RES | st::LANDUSE_OTHER => LANDUSE,
            st::BUILDING => BUILDINGS,
            st::AERO_FILL | st::AERO_LINE => AIRFIELDS,
            st::ROAD_MOTORWAY | st::ROAD_TRUNK => ROADS_MAJOR,
            st::ROAD_PRIMARY | st::ROAD_SECONDARY => ROADS_MAIN,
            st::ROAD_TERTIARY | st::ROAD_MINOR => ROADS_LOCAL,
            st::ROAD_SERVICE => ROADS_SERVICE,
            st::PATH => PATHS,
            st::RAIL => RAIL,
            st::FERRY => FERRIES,
            st::BORDER_COUNTRY => BORDER_COUNTRY,
            st::BORDER_STATE => BORDER_STATE,
            _ => return true,
        };
        self.get(item)
    }

    /// Whether a label of this kind (and place-of-interest group) is shown.
    pub fn label_on(&self, kind: crate::mesh::Kind, cat: u8) -> bool {
        use crate::mesh::Kind;
        self.get(match kind {
            Kind::Continent | Kind::Ocean | Kind::Sea | Kind::Country | Kind::State => L_REGIONS,
            Kind::City | Kind::Town | Kind::Village => L_PLACES,
            Kind::Suburb | Kind::Island => L_AREAS,
            Kind::Street => L_STREETS,
            Kind::HouseNumber => L_HOUSES,
            Kind::Peak => L_PEAKS,
            Kind::Airport => L_AIRPORTS,
            Kind::Poi => POI_FOOD + (cat as usize).min(POI_GROUPS - 1),
        })
    }

    /// The settings-file form: the keys of everything that is switched off.
    pub fn encode(&self) -> String {
        ITEMS.iter().zip(&self.on).filter(|(_, on)| !**on).map(|(i, _)| i.key).collect::<Vec<_>>().join(",")
    }

    /// Reads what `encode` wrote (unknown keys are ignored, so old and new versions can share a file).
    pub fn decode(text: &str) -> MapDetail {
        let mut d = MapDetail::default();
        for key in text.split(',').map(str::trim).filter(|k| !k.is_empty()) {
            if let Some(i) = ITEMS.iter().position(|it| it.key == key) {
                d.on[i] = false;
            }
        }
        d
    }
}

/// How many kinds of place of interest there are (`POI_FOOD..POI_SERVICES`).
pub const POI_GROUPS: usize = POI_SERVICES - POI_FOOD + 1;

/// Which place-of-interest group a tile `class` belongs to, as an index 0..POI_GROUPS.
pub fn poi_group(class: &str) -> u8 {
    match class {
        "restaurant" | "fast_food" | "cafe" | "bar" | "pub" | "bakery" | "ice_cream" | "food_court" | "biergarten" | "alcohol_shop" | "butcher" | "confectionery" | "deli" | "beverages" | "farm" | "food" => 0,
        "shop" | "grocery" | "clothing_store" | "convenience" | "department_store" | "furniture" | "florist" | "jewelry" | "books" | "bicycle" | "electronics" | "hardware" | "mall" | "marketplace" | "hairdresser" | "beauty" | "laundry" | "gift" | "toys" | "sports" | "stationery" | "optician" | "mobile_phone" | "variety_store" | "supermarket" | "garden_centre" | "car_repair" | "car" | "shoes" | "second_hand" | "pet" | "pharmacy_shop" => 1,
        "railway" | "bus" | "subway" | "tram" | "light_rail" | "monorail" | "ferry_terminal" | "bicycle_rental" | "bicycle_parking" | "parking" | "fuel" | "charging_station" | "taxi" | "car_rental" | "car_sharing" | "aerialway" | "airport" | "harbor" | "marina" | "entrance" => 2,
        "hospital" | "pharmacy" | "doctors" | "dentist" | "clinic" | "veterinary" | "social_facility" | "nursing_home" => 3,
        "park" | "playground" | "garden" | "dog_park" | "swimming" | "swimming_pool" | "stadium" | "pitch" | "golf" | "fitness_centre" | "sports_centre" | "recreation_ground" | "picnic_site" | "campsite" | "camp_site" | "caravan_site" | "ski" | "track" | "water_park" | "beach" | "nature_reserve" | "bbq" | "drinking_water" | "shelter" | "viewpoint" | "peak" | "cemetery" | "grave_yard" | "playing_field" => 4,
        "museum" | "theatre" | "cinema" | "art_gallery" | "gallery" | "attraction" | "zoo" | "castle" | "monument" | "memorial" | "ruins" | "place_of_worship" | "library" | "arts_centre" | "theme_park" | "aquarium" | "artwork" | "historic" | "information" | "stadium_sights" | "nightclub" | "casino" | "amusement_arcade" => 5,
        _ => 6,
    }
}

/// Colour of a place-of-interest dot, per group.
pub fn poi_color(group: u8) -> [u8; 3] {
    [
        [226, 108, 40],  // food and drink
        [58, 120, 200],  // shops
        [34, 130, 130],  // transit
        [214, 60, 60],   // health
        [60, 160, 80],   // outdoors
        [150, 80, 180],  // culture
        [110, 110, 125], // services and other
    ][(group as usize).min(6)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique_and_everything_starts_on() {
        let mut seen = std::collections::HashSet::new();
        for it in ITEMS {
            assert!(seen.insert(it.key), "duplicate key {}", it.key);
            assert!(!it.name.is_empty() && !it.group.is_empty());
        }
        let d = MapDetail::default();
        assert!(ITEMS.iter().enumerate().all(|(i, _)| d.get(i)));
        assert_eq!(d.encode(), "");
        assert_eq!(ITEMS.len(), d.on.len());
    }

    #[test]
    fn settings_round_trip_and_ignore_unknown_keys() {
        let mut d = MapDetail::default();
        d.set(BUILDINGS, false);
        d.set(POI_FOOD, false);
        let text = d.encode();
        assert_eq!(text, "buildings,poi_food");
        let back = MapDetail::decode(&format!("{text},some_future_item"));
        assert_eq!(back, d);
        assert!(!back.get(BUILDINGS) && back.get(WATER));
    }

    #[test]
    fn styles_follow_their_switch() {
        let mut d = MapDetail::default();
        assert!(d.style_on(st::BUILDING) && d.style_on(st::ROAD_MINOR));
        d.set(BUILDINGS, false);
        d.set(ROADS_LOCAL, false);
        assert!(!d.style_on(st::BUILDING) && !d.style_on(st::ROAD_MINOR) && !d.style_on(st::ROAD_TERTIARY));
        assert!(d.style_on(st::ROAD_PRIMARY) && d.style_on(st::BG), "the rest stays");
        d.set_all(None, false);
        assert!(!d.style_on(st::WATER) && d.style_on(st::BG), "the ground colour is never switched off");
        d.set_all(Some("Names"), true);
        assert!(d.get(L_STREETS) && !d.get(WATER));
    }

    #[test]
    fn every_style_with_a_switch_is_covered() {
        // each drawn style id maps to some switch except the ground colour
        let mut d = MapDetail::default();
        d.set_all(None, false);
        for id in 1..st::COUNT as u32 {
            assert!(!d.style_on(id), "style {id} has no switch");
        }
    }

    #[test]
    fn labels_follow_their_switch() {
        use crate::mesh::Kind;
        let mut d = MapDetail::default();
        assert!(d.label_on(Kind::Street, 0) && d.label_on(Kind::Poi, 3));
        d.set(L_STREETS, false);
        d.set(POI_HEALTH, false);
        assert!(!d.label_on(Kind::Street, 0));
        assert!(!d.label_on(Kind::Poi, 3) && d.label_on(Kind::Poi, 0));
        assert!(d.label_on(Kind::City, 0));
    }

    #[test]
    fn poi_classes_are_grouped() {
        assert_eq!(poi_group("cafe"), 0);
        assert_eq!(poi_group("clothing_store"), 1);
        assert_eq!(poi_group("bus"), 2);
        assert_eq!(poi_group("hospital"), 3);
        assert_eq!(poi_group("playground"), 4);
        assert_eq!(poi_group("museum"), 5);
        assert_eq!(poi_group("bank"), 6);
        assert_eq!(poi_group("something new"), 6);
        assert_eq!(POI_GROUPS, 7);
    }
}
