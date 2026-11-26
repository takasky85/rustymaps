//! Transit systems as data: what to ask OpenStreetMap (via Overpass) for, and how to turn the
//! answer into lines and stations. Adding another city is one more entry in `SYSTEMS`.
//! Std-only, so it is unit-tested without a GPU or network.

use std::collections::{HashMap, HashSet};

use crate::camera::{dot, lonlat_to_vec, V3};
use crate::json::{self, Json};

pub struct SystemConfig {
    pub id: &'static str,
    pub title: &'static str,
    /// south, west, north, east (degrees).
    pub bbox: [f64; 4],
    /// OSM `route=` values to include.
    pub routes: &'static str,
    /// Extra tag filters; a relation matching any one of them is included.
    pub filters: &'static [&'static str],
    /// (ref, name keyword, colour): standardised line colours, matched by ref or by name.
    pub known: &'static [(&'static str, &'static str, [u8; 3])],
    /// Where to fly to for a look.
    pub home: (f64, f64),
}


const fn sys(
    id: &'static str,
    title: &'static str,
    bbox: [f64; 4],
    routes: &'static str,
    filters: &'static [&'static str],
    known: &'static [(&'static str, &'static str, [u8; 3])],
    home: (f64, f64),
) -> SystemConfig {
    SystemConfig { id, title, bbox, routes, filters, known, home }
}

/// No extra filter: every relation of the wanted `route=` kinds inside the box.
const ANY: &[&str] = &[""];

pub const SYSTEMS: &[SystemConfig] = &[
    sys(
        "muni",
        "SF Muni Metro",
        [37.60, -122.55, 37.84, -122.35],
        "subway|light_rail|tram",
        &[r#"["operator"~"Muni|Municipal Railway|SFMTA"]"#, r#"["network"~"Muni|SFMTA"]"#],
        &[
            ("J", "Church", [247, 148, 29]),
            ("K", "Ingleside", [91, 181, 201]),
            ("L", "Taraval", [146, 39, 143]),
            ("M", "Ocean View", [0, 169, 79]),
            ("N", "Judah", [0, 114, 188]),
            ("T", "Third", [227, 28, 35]),
            ("S", "Shuttle", [255, 194, 14]),
            ("F", "Market", [160, 82, 45]),
        ],
        (37.7897, -122.4003),
    ),
    sys(
        "lametro",
        "Los Angeles Metro",
        [33.72, -118.52, 34.20, -117.85],
        "subway|light_rail",
        &[r#"["network"~"LA Metro|Los Angeles Metro"]"#, r#"["operator"~"LACMTA|Los Angeles County Metropolitan|LA Metro"]"#],
        &[
            ("A", "A Line", [0, 114, 188]),
            ("B", "B Line", [227, 19, 27]),
            ("C", "C Line", [88, 167, 56]),
            ("D", "D Line", [160, 93, 165]),
            ("E", "E Line", [253, 185, 19]),
            ("K", "K Line", [229, 109, 177]),
            ("L", "L Line", [253, 185, 19]),
        ],
        (34.0562, -118.2365),
    ),
    sys(
        "tokyometro",
        "Tokyo Metro",
        [35.55, 139.50, 35.86, 139.95],
        "subway",
        &[r#"["operator"~"Tokyo Metro|東京地下鉄|東京メトロ"]"#, r#"["network"~"Tokyo Metro|東京メトロ"]"#],
        &[
            ("G", "Ginza", [255, 149, 0]),
            ("M", "Marunouchi", [246, 46, 54]),
            ("m", "Marunouchi", [246, 46, 54]),
            ("H", "Hibiya", [181, 181, 172]),
            ("T", "Tozai", [0, 155, 191]),
            ("C", "Chiyoda", [0, 187, 133]),
            ("Y", "Yurakucho", [193, 164, 112]),
            ("Z", "Hanzomon", [143, 118, 214]),
            ("N", "Namboku", [0, 172, 155]),
            ("F", "Fukutoshin", [156, 94, 49]),
        ],
        (35.6812, 139.7671),
    ),
    sys(
        "toei",
        "Tokyo Toei Subway",
        [35.55, 139.50, 35.86, 139.95],
        "subway",
        &[r#"["operator"~"Toei|東京都交通局"]"#, r#"["network"~"Toei|都営"]"#],
        &[("A", "Asakusa", [232, 82, 152]), ("I", "Mita", [0, 121, 194]), ("S", "Shinjuku", [108, 187, 90]), ("E", "Oedo", [182, 0, 122])],
        (35.6896, 139.7006),
    ),
    sys(
        "london",
        "London Underground & DLR",
        [51.28, -0.65, 51.75, 0.30],
        "subway|light_rail",
        &[r#"["network"~"London Underground"]"#, r#"["network"~"Docklands Light Railway"]"#],
        &[
            ("", "bakerloo", [178, 99, 0]),
            ("", "central line", [220, 36, 31]),
            ("", "circle line", [255, 200, 10]),
            ("", "district line", [0, 125, 50]),
            ("", "hammersmith", [244, 169, 190]),
            ("", "jubilee", [134, 143, 152]),
            ("", "metropolitan", [155, 0, 88]),
            ("", "northern line", [20, 20, 20]),
            ("", "piccadilly", [0, 25, 168]),
            ("", "victoria line", [0, 152, 216]),
            ("", "waterloo & city", [147, 206, 186]),
            ("", "docklands", [0, 175, 170]),
        ],
        (51.5074, -0.1278),
    ),
    sys(
        "newyork",
        "New York City Subway",
        [40.50, -74.28, 40.92, -73.70],
        "subway",
        ANY,
        &[
            ("1", "", [238, 53, 46]),
            ("2", "", [238, 53, 46]),
            ("3", "", [238, 53, 46]),
            ("4", "", [0, 147, 60]),
            ("5", "", [0, 147, 60]),
            ("6", "", [0, 147, 60]),
            ("7", "", [185, 51, 173]),
            ("A", "", [0, 57, 166]),
            ("C", "", [0, 57, 166]),
            ("E", "", [0, 57, 166]),
            ("B", "", [255, 99, 25]),
            ("D", "", [255, 99, 25]),
            ("F", "", [255, 99, 25]),
            ("M", "", [255, 99, 25]),
            ("G", "", [108, 190, 69]),
            ("J", "", [153, 102, 51]),
            ("Z", "", [153, 102, 51]),
            ("L", "", [167, 169, 172]),
            ("N", "", [252, 204, 10]),
            ("Q", "", [252, 204, 10]),
            ("R", "", [252, 204, 10]),
            ("W", "", [252, 204, 10]),
            ("S", "", [128, 129, 131]),
        ],
        (40.7527, -73.9772),
    ),
    sys(
        "paris",
        "Paris Métro",
        [48.70, 2.10, 49.00, 2.60],
        "subway",
        ANY,
        &[
            ("1", "", [255, 205, 0]),
            ("2", "", [0, 60, 166]),
            ("3", "", [131, 121, 2]),
            ("3bis", "", [110, 196, 232]),
            ("4", "", [207, 0, 158]),
            ("5", "", [255, 126, 46]),
            ("6", "", [110, 202, 151]),
            ("7", "", [250, 154, 186]),
            ("7bis", "", [110, 202, 151]),
            ("8", "", [225, 155, 223]),
            ("9", "", [182, 189, 0]),
            ("10", "", [201, 145, 13]),
            ("11", "", [112, 75, 28]),
            ("12", "", [0, 120, 82]),
            ("13", "", [110, 196, 232]),
            ("14", "", [98, 37, 157]),
        ],
        (48.8566, 2.3522),
    ),
    sys("moscow", "Moscow Metro", [55.55, 37.30, 55.95, 37.95], "subway|light_rail", ANY, &[], (55.7558, 37.6173)),
    sys("beijing", "Beijing Subway", [39.70, 116.05, 40.25, 116.75], "subway", ANY, &[], (39.9087, 116.3975)),
    sys("shanghai", "Shanghai Metro", [30.90, 121.20, 31.45, 121.85], "subway", ANY, &[], (31.2304, 121.4737)),
    sys("guangzhou", "Guangzhou Metro", [22.95, 113.10, 23.35, 113.55], "subway", ANY, &[], (23.1291, 113.2644)),
    sys("shenzhen", "Shenzhen Metro", [22.45, 113.80, 22.80, 114.35], "subway", ANY, &[], (22.5431, 114.0579)),
    sys("chengdu", "Chengdu Metro", [30.45, 103.85, 30.85, 104.30], "subway", ANY, &[], (30.5728, 104.0668)),
    sys("wuhan", "Wuhan Metro", [30.35, 114.05, 30.75, 114.55], "subway", ANY, &[], (30.5928, 114.3055)),
    sys("nanjing", "Nanjing Metro", [31.85, 118.50, 32.25, 119.05], "subway", ANY, &[], (32.0603, 118.7969)),
    sys("hangzhou", "Hangzhou Metro", [30.10, 120.00, 30.40, 120.45], "subway", ANY, &[], (30.2741, 120.1551)),
    sys("osaka", "Osaka Metro", [34.45, 135.30, 34.85, 135.70], "subway", ANY, &[], (34.7025, 135.4959)),
    sys("seoul", "Seoul Metropolitan Subway", [37.35, 126.70, 37.75, 127.30], "subway", ANY, &[], (37.5547, 126.9707)),
    sys("delhi", "Delhi Metro", [28.35, 76.85, 28.90, 77.50], "subway", ANY, &[], (28.6328, 77.2197)),
    sys("hongkong", "Hong Kong MTR", [22.20, 113.90, 22.45, 114.30], "subway", ANY, &[], (22.2819, 114.1583)),
    sys("singapore", "Singapore MRT & LRT", [1.22, 103.60, 1.48, 104.05], "subway|light_rail", ANY, &[], (1.2830, 103.8513)),
    sys("mexicocity", "Mexico City Metro", [19.30, -99.30, 19.55, -98.95], "subway", ANY, &[], (19.4326, -99.1332)),
    sys("madrid", "Madrid Metro", [40.25, -3.90, 40.60, -3.45], "subway|light_rail", ANY, &[], (40.4168, -3.7038)),
    sys("berlin", "Berlin U-Bahn", [52.35, 13.05, 52.65, 13.80], "subway", ANY, &[], (52.5200, 13.4050)),
    sys("cairo", "Cairo Metro", [29.95, 31.15, 30.20, 31.50], "subway", ANY, &[], (30.0444, 31.2357)),
    sys("saopaulo", "São Paulo Metro", [-23.75, -46.85, -23.40, -46.45], "subway", ANY, &[], (-23.5505, -46.6333)),

    // ---- Japan: regional and private railways around Tokyo (the Yamanote line is JR East) ----
    sys(
        "jreast",
        "JR East (Kanto)",
        [35.10, 139.05, 36.15, 140.40],
        "train|subway|light_rail",
        &[r#"["operator"~"JR East|東日本旅客鉄道"]"#, r#"["network"~"JR East|JR東日本"]"#],
        &[
            ("", "yamanote", [154, 205, 50]),
            ("", "山手線", [154, 205, 50]),
            ("", "keihin-tohoku", [0, 178, 229]),
            ("", "京浜東北線", [0, 178, 229]),
            ("", "chuo line (rapid)", [240, 86, 37]),
            ("", "中央線(快速)", [240, 86, 37]),
            ("", "chuo-sobu", [255, 212, 0]),
            ("", "中央・総武", [255, 212, 0]),
            ("", "saikyo", [0, 172, 154]),
            ("", "埼京線", [0, 172, 154]),
            ("", "joban", [0, 176, 107]),
            ("", "常磐線", [0, 176, 107]),
            ("", "musashino", [240, 86, 37]),
            ("", "武蔵野線", [240, 86, 37]),
            ("", "keiyo", [200, 30, 40]),
            ("", "京葉線", [200, 30, 40]),
            ("", "nambu", [255, 212, 0]),
            ("", "南武線", [255, 212, 0]),
            ("", "tokaido", [240, 120, 40]),
            ("", "東海道線", [240, 120, 40]),
        ],
        (35.6812, 139.7671),
    ),
    sys(
        "tokyoprivate",
        "Tokyo private railways",
        [35.20, 139.20, 36.10, 140.30],
        "train|subway|light_rail",
        &[
            r#"["operator"~"Tokyu|東急|Odakyu|小田急|Keio|京王|Seibu|西武|Tobu|東武|Keikyu|京急|Keisei|京成|Sotetsu|相鉄|Tsukuba Express|首都圏新都市鉄道|Yurikamome|ゆりかもめ|Rinkai|東京臨海高速鉄道|Tokyo Monorail|東京モノレール"]"#,
        ],
        &[],
        (35.6586, 139.7454),
    ),
    sys("yokohama", "Yokohama Subway", [35.30, 139.40, 35.65, 139.75], "subway|light_rail", ANY, &[], (35.4660, 139.6226)),
    sys("nagoya", "Nagoya Subway", [34.95, 136.75, 35.35, 137.15], "subway|light_rail", ANY, &[], (35.1709, 136.8815)),
    sys("kyoto", "Kyoto Subway", [34.85, 135.65, 35.15, 135.90], "subway", ANY, &[], (35.0116, 135.7681)),
    sys("fukuoka", "Fukuoka Subway", [33.50, 130.25, 33.75, 130.55], "subway", ANY, &[], (33.5902, 130.4207)),
    sys("sapporo", "Sapporo Subway", [42.90, 141.15, 43.20, 141.55], "subway", ANY, &[], (43.0687, 141.3508)),
    sys(
        "jrwest",
        "JR West (Kansai)",
        [34.45, 135.10, 35.15, 135.95],
        "train|subway|light_rail",
        &[r#"["operator"~"JR West|西日本旅客鉄道"]"#, r#"["network"~"JR West|JR西日本"]"#],
        &[],
        (34.7025, 135.4959),
    ),
    // ---- United States and Canada ----
    sys("chicago", "Chicago L", [41.65, -88.0, 42.15, -87.5], "subway|light_rail", ANY, &[], (41.8819, -87.6278)),
    sys("boston", "Boston MBTA", [42.15, -71.30, 42.55, -70.90], "subway|light_rail", ANY, &[], (42.3555, -71.0603)),
    sys("washington", "Washington Metro", [38.70, -77.40, 39.10, -76.80], "subway", ANY, &[], (38.8983, -77.0280)),
    sys("philadelphia", "Philadelphia SEPTA", [39.85, -75.40, 40.15, -75.0], "subway|light_rail", ANY, &[], (39.9526, -75.1652)),
    sys("atlanta", "Atlanta MARTA", [33.60, -84.65, 33.95, -84.20], "subway|light_rail", ANY, &[], (33.7490, -84.3880)),
    sys("seattle", "Seattle Link", [47.40, -122.45, 47.85, -122.15], "subway|light_rail", ANY, &[], (47.6097, -122.3331)),
    sys("portland", "Portland MAX", [45.40, -123.0, 45.65, -122.35], "light_rail", ANY, &[], (45.5202, -122.6742)),
    sys("sandiego", "San Diego Trolley", [32.50, -117.30, 33.0, -116.90], "light_rail", ANY, &[], (32.7157, -117.1611)),
    sys("bart", "San Francisco BART", [37.30, -122.55, 38.05, -121.75], "subway", ANY, &[], (37.7929, -122.3969)),
    sys(
        "caltrain",
        "Caltrain & Bay Area rail",
        [36.95, -122.55, 38.0, -121.55],
        "train",
        &[r#"["network"~"Caltrain|ACE|SMART|Capitol Corridor"]"#, r#"["operator"~"Caltrain|Peninsula Corridor"]"#],
        &[],
        (37.7766, -122.3948),
    ),
    sys("denver", "Denver RTD", [39.45, -105.25, 40.05, -104.65], "light_rail|train", &[r#"["network"~"RTD|Regional Transportation District"]"#], &[], (39.7527, -104.9998)),
    sys("dallas", "Dallas DART", [32.55, -97.30, 33.25, -96.45], "light_rail|train", &[r#"["network"~"DART|Trinity Railway"]"#], &[], (32.7791, -96.7970)),
    sys("houston", "Houston METRORail", [29.55, -95.55, 29.90, -95.20], "light_rail", ANY, &[], (29.7604, -95.3698)),
    sys("miami", "Miami Metrorail", [25.55, -80.50, 26.0, -80.10], "subway|light_rail", ANY, &[], (25.7743, -80.1937)),
    sys(
        "nycrail",
        "NYC commuter rail & PATH",
        [40.40, -74.60, 41.50, -73.10],
        "train|subway",
        &[r#"["network"~"Metro-North|Long Island Rail Road|LIRR|PATH|NJ Transit"]"#, r#"["operator"~"Metro-North|Long Island Rail Road|PATH|NJ Transit"]"#],
        &[],
        (40.7527, -73.9772),
    ),
    sys(
        "metra",
        "Chicago Metra",
        [41.40, -88.50, 42.55, -87.50],
        "train",
        &[r#"["operator"~"Metra"]"#, r#"["network"~"Metra"]"#],
        &[],
        (41.8786, -87.6406),
    ),
    sys(
        "mbtarail",
        "Boston commuter rail",
        [41.85, -71.65, 42.95, -70.65],
        "train",
        &[r#"["network"~"MBTA"]"#, r#"["operator"~"MBTA|Keolis"]"#],
        &[],
        (42.3519, -71.0552),
    ),
    sys(
        "metrolink",
        "LA Metrolink",
        [33.40, -118.65, 34.65, -116.90],
        "train",
        &[r#"["operator"~"Metrolink|Southern California Regional Rail"]"#, r#"["network"~"Metrolink"]"#],
        &[],
        (34.0561, -118.2365),
    ),
    sys("toronto", "Toronto TTC", [43.55, -79.70, 43.90, -79.10], "subway|light_rail", ANY, &[], (43.6453, -79.3806)),
    sys("montreal", "Montréal Métro", [45.40, -73.90, 45.70, -73.45], "subway|light_rail", ANY, &[], (45.5017, -73.5673)),
    sys("vancouver", "Vancouver SkyTrain", [49.0, -123.25, 49.35, -122.65], "subway|light_rail", ANY, &[], (49.2827, -123.1207)),
    // ---- Europe ----
    sys(
        "londonrail",
        "London Overground & Elizabeth line",
        [51.28, -0.65, 51.75, 0.35],
        "train|light_rail|subway",
        &[r#"["network"~"London Overground|Elizabeth line|TfL Rail"]"#, r#"["operator"~"London Overground|Transport for London|TfL Rail"]["route"="train"]"#],
        &[],
        (51.5203, -0.0753),
    ),
    sys(
        "pariszrail",
        "Paris RER & Transilien",
        [48.45, 1.90, 49.15, 2.90],
        "train|light_rail",
        &[r#"["network"~"RER|Transilien"]"#],
        &[],
        (48.8809, 2.3553),
    ),
    sys("berlinsbahn", "Berlin S-Bahn", [52.20, 12.90, 52.75, 13.95], "light_rail", ANY, &[], (52.5251, 13.3694)),
    sys("munich", "Munich U- & S-Bahn", [47.95, 11.30, 48.35, 11.85], "subway|light_rail", ANY, &[], (48.1402, 11.5586)),
    sys("hamburg", "Hamburg U- & S-Bahn", [53.35, 9.70, 53.75, 10.30], "subway|light_rail", ANY, &[], (53.5530, 10.0067)),
    sys("frankfurt", "Frankfurt U- & S-Bahn", [49.90, 8.45, 50.30, 8.95], "subway|light_rail", ANY, &[], (50.1070, 8.6638)),
    sys("cologne", "Cologne & Bonn Stadtbahn", [50.70, 6.75, 51.10, 7.35], "subway|light_rail", ANY, &[], (50.9430, 6.9589)),
    sys("vienna", "Vienna U-Bahn", [48.10, 16.15, 48.35, 16.60], "subway", ANY, &[], (48.2009, 16.3689)),
    sys("prague", "Prague Metro", [49.95, 14.25, 50.20, 14.70], "subway", ANY, &[], (50.0833, 14.4349)),
    sys("budapest", "Budapest Metro", [47.35, 18.90, 47.65, 19.30], "subway|light_rail", ANY, &[], (47.4979, 19.0402)),
    sys("warsaw", "Warsaw Metro", [52.05, 20.80, 52.40, 21.25], "subway", ANY, &[], (52.2297, 21.0122)),
    sys("amsterdam", "Amsterdam Metro", [52.25, 4.70, 52.45, 5.05], "subway|light_rail", ANY, &[], (52.3791, 4.9003)),
    sys("brussels", "Brussels Metro", [50.75, 4.20, 50.95, 4.55], "subway|light_rail", ANY, &[], (50.8503, 4.3517)),
    sys("copenhagen", "Copenhagen Metro & S-tog", [55.50, 12.25, 55.90, 12.75], "subway|light_rail|train", &[r#"["network"~"Metro|S-tog|Metroen"]"#], &[], (55.6761, 12.5683)),
    sys("stockholm", "Stockholm Tunnelbana", [59.15, 17.70, 59.55, 18.30], "subway|light_rail", ANY, &[], (59.3326, 18.0649)),
    sys("oslo", "Oslo T-bane", [59.80, 10.45, 60.10, 11.0], "subway|light_rail", ANY, &[], (59.9139, 10.7522)),
    sys("helsinki", "Helsinki Metro", [60.10, 24.60, 60.35, 25.20], "subway", ANY, &[], (60.1699, 24.9384)),
    sys("milan", "Milan Metro", [45.30, 9.0, 45.65, 9.40], "subway", ANY, &[], (45.4642, 9.1900)),
    sys("rome", "Rome Metro", [41.75, 12.30, 42.05, 12.70], "subway", ANY, &[], (41.9028, 12.4964)),
    sys("barcelona", "Barcelona Metro", [41.30, 2.0, 41.55, 2.35], "subway|light_rail", ANY, &[], (41.3874, 2.1686)),
    sys("lisbon", "Lisbon Metro", [38.65, -9.30, 38.85, -9.05], "subway", ANY, &[], (38.7223, -9.1393)),
    sys("athens", "Athens Metro", [37.85, 23.55, 38.20, 24.0], "subway|light_rail", ANY, &[], (37.9838, 23.7275)),
    sys("istanbul", "Istanbul Metro", [40.85, 28.55, 41.30, 29.45], "subway|light_rail", ANY, &[], (41.0082, 28.9784)),
    sys("kyiv", "Kyiv Metro", [50.30, 30.30, 50.60, 30.85], "subway", ANY, &[], (50.4501, 30.5234)),
    sys("stpetersburg", "Saint Petersburg Metro", [59.75, 30.05, 60.10, 30.55], "subway", ANY, &[], (59.9311, 30.3609)),
    // ---- Asia, Middle East, Oceania, Latin America ----
    sys("bangkok", "Bangkok BTS & MRT", [13.45, 100.30, 14.10, 100.95], "subway|light_rail", ANY, &[], (13.7563, 100.5018)),
    sys("taipei", "Taipei Metro", [24.85, 121.35, 25.25, 121.75], "subway|light_rail", ANY, &[], (25.0478, 121.5170)),
    sys("kualalumpur", "Kuala Lumpur Rail", [2.90, 101.40, 3.35, 101.90], "subway|light_rail", ANY, &[], (3.1390, 101.6869)),
    sys("jakarta", "Jakarta MRT & LRT", [-6.40, 106.65, -6.10, 107.05], "subway|light_rail", ANY, &[], (-6.2088, 106.8456)),
    sys("manila", "Manila LRT & MRT", [14.40, 120.90, 14.80, 121.20], "subway|light_rail", ANY, &[], (14.5995, 120.9842)),
    sys("mumbai", "Mumbai Metro", [18.85, 72.75, 19.40, 73.20], "subway|light_rail", ANY, &[], (19.0760, 72.8777)),
    sys("kolkata", "Kolkata Metro", [22.40, 88.20, 22.75, 88.50], "subway", ANY, &[], (22.5726, 88.3639)),
    sys("bangalore", "Bengaluru Metro", [12.80, 77.40, 13.15, 77.85], "subway", ANY, &[], (12.9716, 77.5946)),
    sys("dubai", "Dubai Metro", [24.95, 55.00, 25.40, 55.50], "subway|light_rail", ANY, &[], (25.2048, 55.2708)),
    sys("busan", "Busan Metro", [34.95, 128.85, 35.35, 129.30], "subway|light_rail", ANY, &[], (35.1796, 129.0756)),
    sys("tianjin", "Tianjin Metro", [38.85, 117.00, 39.35, 117.55], "subway", ANY, &[], (39.3434, 117.3616)),
    sys("chongqing", "Chongqing Rail", [29.30, 106.30, 29.85, 106.95], "subway|light_rail", ANY, &[], (29.5630, 106.5516)),
    sys("xian", "Xi'an Metro", [34.10, 108.75, 34.50, 109.15], "subway", ANY, &[], (34.3416, 108.9398)),
    sys(
        "sydney",
        "Sydney Trains & Metro",
        [-34.35, 150.60, -33.55, 151.45],
        "train|subway|light_rail",
        &[r#"["network"~"Sydney Trains|Sydney Metro|NSW TrainLink"]["route"!="bus"]"#, r#"["operator"~"Sydney Trains|Sydney Metro"]"#],
        &[],
        (-33.8830, 151.2060),
    ),
    sys(
        "melbourne",
        "Melbourne trains",
        [-38.40, 144.45, -37.50, 145.50],
        "train",
        &[r#"["network"~"Metro Trains Melbourne|Metro Trains"]"#, r#"["operator"~"Metro Trains Melbourne"]"#],
        &[],
        (-37.8183, 144.9671),
    ),
    sys(
        "brisbane",
        "Brisbane trains",
        [-27.95, 152.60, -27.0, 153.35],
        "train|light_rail",
        &[r#"["network"~"Queensland Rail|Translink|TransLink"]"#, r#"["operator"~"Queensland Rail|TransLink"]"#],
        &[],
        (-27.4698, 153.0251),
    ),
    sys(
        "perth",
        "Perth Transperth",
        [-32.60, 115.55, -31.50, 116.15],
        "train",
        &[r#"["network"~"Transperth"]"#, r#"["operator"~"Transperth|Public Transport Authority"]"#],
        &[],
        (-31.9505, 115.8605),
    ),
    sys("santiago", "Santiago Metro", [-33.70, -70.85, -33.30, -70.45], "subway", ANY, &[], (-33.4372, -70.6506)),
    sys("buenosaires", "Buenos Aires Subte", [-34.75, -58.60, -34.50, -58.30], "subway|light_rail", ANY, &[], (-34.6037, -58.3816)),
    sys("rio", "Rio de Janeiro Metro", [-23.10, -43.45, -22.80, -43.10], "subway|light_rail", ANY, &[], (-22.9068, -43.1729)),
];

pub struct TLine {
    pub name: String,
    /// Short letter/number for the end-of-line roundel.
    pub badge: String,
    pub color: [u8; 3],
    /// Unit-sphere polylines following the real track.
    pub paths: Vec<Vec<V3>>,
    /// Distances along the line's main path, for moving trains (filled in by `finish`).
    pub track: Track,
}

/// Cumulative arc length (radians) along `paths[path]`, plus where trains stop.
#[derive(Default)]
pub struct Track {
    pub path: usize,
    pub cum: Vec<f64>,
    /// Arc length of each station on the track, ascending (the ends if none were found).
    pub stops: Vec<f64>,
    /// One timetable cycle: out along the line and back, stopping at every station.
    legs: Vec<Leg>,
    cycle: f64,
}

/// One stop-and-go leg of a train's cycle: wait `dwell` s at `from`, then drive to `to`.
struct Leg {
    start: f64,
    from: f64,
    to: f64,
    dwell: f64,
    travel: f64,
}

pub const EARTH_RADIUS_M: f64 = 6_371_000.0;
/// Top speed, acceleration and station stop of the simulated trains (a typical metro).
pub const TRAIN_VMAX: f64 = 18.0;
const TRAIN_ACC: f64 = 1.0;
const TRAIN_DWELL: f64 = 25.0;

/// Seconds to drive `d` metres from rest to rest: accelerate, cruise, brake.
fn drive_time(d: f64) -> f64 {
    let d_acc = TRAIN_VMAX * TRAIN_VMAX / (2.0 * TRAIN_ACC);
    if d >= 2.0 * d_acc {
        2.0 * TRAIN_VMAX / TRAIN_ACC + (d - 2.0 * d_acc) / TRAIN_VMAX
    } else {
        2.0 * (d / TRAIN_ACC).sqrt()
    }
}

/// Fraction (0..=1) of the way after `t` seconds of a drive of `d` metres that takes `total` s.
fn drive_fraction(d: f64, total: f64, t: f64) -> f64 {
    if d <= 0.0 || total <= 0.0 {
        return 1.0;
    }
    let vpeak = TRAIN_VMAX.min((TRAIN_ACC * d).sqrt());
    let t_acc = vpeak / TRAIN_ACC;
    let d_acc = vpeak * vpeak / (2.0 * TRAIN_ACC);
    let t = t.clamp(0.0, total);
    let pos = if t < t_acc {
        0.5 * TRAIN_ACC * t * t
    } else if t < total - t_acc {
        d_acc + vpeak * (t - t_acc)
    } else {
        let rem = total - t;
        d - 0.5 * TRAIN_ACC * rem * rem
    };
    (pos / d).clamp(0.0, 1.0)
}

impl Track {
    /// The point at arc length `s` along the track (clamped to its ends).
    pub fn point_at(&self, paths: &[Vec<V3>], s: f64) -> Option<V3> {
        let path = paths.get(self.path)?;
        if self.cum.len() < 2 || self.cum.len() != path.len() {
            return None;
        }
        let i = self.cum.partition_point(|&c| c < s).clamp(1, self.cum.len() - 1);
        let (a, b) = (i - 1, i);
        let span = (self.cum[b] - self.cum[a]).max(1e-15);
        let t = ((s - self.cum[a]) / span).clamp(0.0, 1.0);
        let (p, q) = (path[a], path[b]);
        Some(crate::camera::normalize([p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t, p[2] + (q[2] - p[2]) * t]))
    }

    pub fn length(&self) -> f64 {
        self.cum.last().copied().unwrap_or(0.0)
    }

    /// Seconds for a train to go out and come back, with every stop.
    pub fn cycle_seconds(&self) -> f64 {
        self.cycle
    }

    /// Arc length (radians) of a train `t` seconds into its cycle: it accelerates, cruises, brakes,
    /// waits at each station, and turns round at the ends.
    pub fn s_at(&self, t: f64) -> f64 {
        if self.legs.is_empty() || self.cycle <= 0.0 {
            return 0.0;
        }
        let t = t.rem_euclid(self.cycle);
        let i = self.legs.partition_point(|l| l.start <= t).saturating_sub(1);
        let l = &self.legs[i];
        let into = t - l.start;
        if into < l.dwell {
            return l.from;
        }
        let d = (l.to - l.from).abs() * EARTH_RADIUS_M;
        l.from + (l.to - l.from) * drive_fraction(d, l.travel, into - l.dwell)
    }

    /// Builds the timetable from the stop positions.
    fn schedule(&mut self) {
        self.legs.clear();
        self.cycle = 0.0;
        if self.stops.len() < 2 {
            return;
        }
        let n = self.stops.len();
        let order: Vec<usize> = (0..n).chain((0..n - 1).rev()).collect();
        let mut t = 0.0;
        for w in order.windows(2).chain(std::iter::once(&[order[n * 2 - 2], order[0]][..])) {
            let (from, to) = (self.stops[w[0]], self.stops[w[1]]);
            let travel = drive_time((to - from).abs() * EARTH_RADIUS_M);
            self.legs.push(Leg { start: t, from, to, dwell: TRAIN_DWELL, travel });
            t += TRAIN_DWELL + travel;
        }
        self.cycle = t;
    }
}

pub struct Station {
    pub name: String,
    pub pos: V3,
    /// Indices into `SystemData::lines`; two or more means an interchange.
    pub lines: Vec<usize>,
}

#[derive(Default)]
pub struct SystemData {
    pub lines: Vec<TLine>,
    pub stations: Vec<Station>,
    /// Unit vector at the middle of everything, and the angular radius (radians) that contains it.
    pub center: V3,
    pub radius: f64,
}

impl SystemData {
    /// Computes `center` and `radius`; call after the lines are filled in.
    pub fn finish(&mut self) {
        // The main path of each line (the one the end-of-line roundels sit on) is where trains run.
        for (li, line) in self.lines.iter_mut().enumerate() {
            let Some((pi, path)) = line.paths.iter().enumerate().max_by_key(|(_, p)| p.len()) else { continue };
            let mut cum = Vec::with_capacity(path.len());
            let mut acc = 0.0f64;
            for (k, p) in path.iter().enumerate() {
                if k > 0 {
                    acc += dot(path[k - 1], *p).clamp(-1.0, 1.0).acos();
                }
                cum.push(acc);
            }
            // Where this line's stations sit along its main path (within 250 m of it).
            let mut stops: Vec<f64> = Vec::new();
            for st in self.stations.iter().filter(|st| st.lines.contains(&li)) {
                let mut best = (f64::MAX, 0.0f64);
                for k in 1..path.len() {
                    let (p, q) = (path[k - 1], path[k]);
                    let d = [q[0] - p[0], q[1] - p[1], q[2] - p[2]];
                    let len2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
                    let x = [st.pos[0] - p[0], st.pos[1] - p[1], st.pos[2] - p[2]];
                    let t = if len2 <= 1e-30 { 0.0 } else { ((x[0] * d[0] + x[1] * d[1] + x[2] * d[2]) / len2).clamp(0.0, 1.0) };
                    let e = [x[0] - d[0] * t, x[1] - d[1] * t, x[2] - d[2] * t];
                    let dist = (e[0] * e[0] + e[1] * e[1] + e[2] * e[2]).sqrt();
                    if dist < best.0 {
                        best = (dist, cum[k - 1] + (cum[k] - cum[k - 1]) * t);
                    }
                }
                if best.0 * EARTH_RADIUS_M < 250.0 {
                    stops.push(best.1);
                }
            }
            stops.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            // one stop per station (platforms of one station can be a few metres apart)
            stops.dedup_by(|b, a| (*b - *a) * EARTH_RADIUS_M < 150.0);
            let end = cum.last().copied().unwrap_or(0.0);
            if stops.len() < 2 {
                stops = vec![0.0, end];
            }
            line.track = Track { path: pi, cum, stops, ..Default::default() };
            line.track.schedule();
        }
        let mut sum = [0.0f64; 3];
        let mut n = 0usize;
        for p in self.lines.iter().flat_map(|l| l.paths.iter()).flat_map(|p| p.iter().step_by(4)) {
            sum = [sum[0] + p[0], sum[1] + p[1], sum[2] + p[2]];
            n += 1;
        }
        if n == 0 {
            self.center = [0.0, 0.0, 1.0];
            self.radius = 0.0;
            return;
        }
        let c = crate::camera::normalize(sum);
        let mut worst = 0.0f64;
        for p in self.lines.iter().flat_map(|l| l.paths.iter()).flat_map(|p| p.iter()) {
            worst = worst.max(dot(c, *p).clamp(-1.0, 1.0).acos());
        }
        self.center = c;
        self.radius = worst;
    }

    pub fn point_count(&self) -> usize {
        self.lines.iter().flat_map(|l| l.paths.iter()).map(|p| p.len()).sum()
    }
}

// ---------------------------------------------------------------------------
// Compact on-disk format (`<id>.tmap`). The raw Overpass answer is tens of megabytes per big city;
// this keeps only what is drawn, at about 1 cm precision.
// ---------------------------------------------------------------------------

const TMAP_MAGIC: &[u8; 4] = b"TMP1";

fn put_u16(o: &mut Vec<u8>, v: usize) {
    o.extend_from_slice(&(v.min(u16::MAX as usize) as u16).to_le_bytes());
}
fn put_u32(o: &mut Vec<u8>, v: usize) {
    o.extend_from_slice(&(v as u32).to_le_bytes());
}
fn put_str(o: &mut Vec<u8>, s: &str) {
    let mut end = s.len().min(200);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    o.push(end as u8);
    o.extend_from_slice(&s.as_bytes()[..end]);
}
fn put_ll(o: &mut Vec<u8>, p: V3) {
    let (lon, lat) = crate::camera::vec_to_lonlat(p);
    o.extend_from_slice(&((lon.to_degrees() * 1e7).round() as i32).to_le_bytes());
    o.extend_from_slice(&((lat.to_degrees() * 1e7).round() as i32).to_le_bytes());
}

pub fn encode(d: &SystemData) -> Vec<u8> {
    let mut o = Vec::with_capacity(d.point_count() * 8 + 4096);
    o.extend_from_slice(TMAP_MAGIC);
    put_u16(&mut o, d.lines.len());
    for l in &d.lines {
        o.extend_from_slice(&l.color);
        put_str(&mut o, &l.badge);
        put_str(&mut o, &l.name);
        put_u32(&mut o, l.paths.len());
        for p in &l.paths {
            put_u32(&mut o, p.len());
            for &v in p {
                put_ll(&mut o, v);
            }
        }
    }
    put_u32(&mut o, d.stations.len());
    for s in &d.stations {
        put_str(&mut o, &s.name);
        put_ll(&mut o, s.pos);
        o.push(s.lines.len().min(255) as u8);
        for &li in s.lines.iter().take(255) {
            put_u16(&mut o, li);
        }
    }
    o
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.b.len()).ok_or("file ends early")?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<usize, String> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]) as usize)
    }
    fn u32(&mut self) -> Result<usize, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    }
    fn i32(&mut self) -> Result<i32, String> {
        let b = self.take(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn string(&mut self) -> Result<String, String> {
        let n = self.u8()? as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
    fn ll(&mut self) -> Result<V3, String> {
        let lon = self.i32()? as f64 * 1e-7;
        let lat = self.i32()? as f64 * 1e-7;
        Ok(lonlat_to_vec(lon.to_radians(), lat.to_radians()))
    }
}

pub fn decode(bytes: &[u8]) -> Result<SystemData, String> {
    if bytes.len() < 4 || &bytes[..4] != TMAP_MAGIC {
        return Err("not a RustyMaps transit file".into());
    }
    let mut r = Reader { b: bytes, at: 4 };
    let mut d = SystemData::default();
    for _ in 0..r.u16()? {
        let c = r.take(3)?;
        let color = [c[0], c[1], c[2]];
        let badge = r.string()?;
        let name = r.string()?;
        let mut paths = Vec::new();
        for _ in 0..r.u32()? {
            let n = r.u32()?;
            if n > 5_000_000 {
                return Err("corrupt file".into());
            }
            let mut p = Vec::with_capacity(n);
            for _ in 0..n {
                p.push(r.ll()?);
            }
            paths.push(p);
        }
        d.lines.push(TLine { name, badge, color, paths, track: Track::default() });
    }
    for _ in 0..r.u32()? {
        let name = r.string()?;
        let pos = r.ll()?;
        let n = r.u8()? as usize;
        let mut lines = Vec::with_capacity(n);
        for _ in 0..n {
            lines.push(r.u16()?);
        }
        d.stations.push(Station { name, pos, lines });
    }
    d.finish();
    Ok(d)
}

/// Overpass QL for one system: route relations with full geometry, then their stop nodes.
pub fn overpass_query(c: &SystemConfig) -> String {
    let [s, w, n, e] = c.bbox;
    let mut q = String::from("[out:json][timeout:120];(");
    for f in c.filters {
        q.push_str(&format!("relation[\"route\"~\"^({})$\"]{f}({s},{w},{n},{e});", c.routes));
    }
    q.push_str(")->.r;.r out geom;(node(r.r:\"stop\");node(r.r:\"stop_entry_only\");node(r.r:\"stop_exit_only\"););out;");
    q
}

/// Percent-encodes `s` for use in a URL query value.
pub fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn parse_color(s: &str) -> Option<[u8; 3]> {
    let t = s.trim();
    if let Some(h) = t.strip_prefix('#') {
        let h = if h.len() == 3 { h.chars().flat_map(|c| [c, c]).collect::<String>() } else { h.to_string() };
        if h.len() == 6 {
            let v = u32::from_str_radix(&h, 16).ok()?;
            return Some([(v >> 16) as u8, (v >> 8) as u8, v as u8]);
        }
        return None;
    }
    Some(match t.to_ascii_lowercase().as_str() {
        "red" => [220, 40, 40],
        "orange" => [247, 148, 29],
        "yellow" => [250, 200, 20],
        "green" => [40, 160, 70],
        "blue" => [30, 100, 200],
        "purple" => [140, 70, 160],
        "pink" => [230, 110, 170],
        "brown" => [150, 90, 50],
        "gray" | "grey" => [150, 150, 150],
        "black" => [40, 40, 40],
        "teal" => [0, 150, 150],
        _ => return None,
    })
}

fn hash_color(key: &str) -> [u8; 3] {
    let mut h: u32 = 2166136261;
    for b in key.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    let hue = (h % 360) as f32 / 60.0;
    let x = 1.0 - (hue % 2.0 - 1.0).abs();
    let (r, g, b) = match hue as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    let f = |v: f32| (30.0 + v * 190.0) as u8;
    [f(r), f(g), f(b)]
}

fn is_latin(s: &str) -> bool {
    s.chars().all(|c| c.is_ascii() || ('\u{00C0}'..='\u{024F}').contains(&c) || c == '\u{2019}' || c == '\u{2013}')
}

/// English/Latin display name from an OSM `tags` object ("" if there is none).
fn latin_name(tags: &Json) -> String {
    for k in ["name:en", "name:latin", "int_name", "name"] {
        if let Some(n) = tags.str_at(k) {
            if is_latin(n) && !n.is_empty() {
                return n.to_string();
            }
        }
    }
    String::new()
}

type LonLat = (f64, f64);

fn key_of(p: LonLat) -> (i64, i64) {
    ((p.0 * 1e7).round() as i64, (p.1 * 1e7).round() as i64)
}

/// Joins ways that share end points into longer polylines (so line joins render smoothly).
pub fn chain(ways: Vec<Vec<LonLat>>) -> Vec<Vec<LonLat>> {
    let mut at: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, w) in ways.iter().enumerate() {
        if w.len() < 2 {
            continue;
        }
        at.entry(key_of(w[0])).or_default().push(i);
        at.entry(key_of(*w.last().unwrap())).or_default().push(i);
    }
    let mut used = vec![false; ways.len()];
    let mut out = Vec::new();
    for start in 0..ways.len() {
        if used[start] || ways[start].len() < 2 {
            continue;
        }
        used[start] = true;
        let mut line = ways[start].clone();
        // grow at the tail, then at the head
        for tail in [true, false] {
            loop {
                let end = if tail { *line.last().unwrap() } else { line[0] };
                let next = at.get(&key_of(end)).and_then(|v| v.iter().copied().find(|&j| !used[j]));
                let Some(j) = next else { break };
                used[j] = true;
                let mut w = ways[j].clone();
                // orient so that w begins at `end` (tail) or finishes at `end` (head)
                let begins_here = key_of(w[0]) == key_of(end);
                if tail == begins_here {
                    if tail {
                        line.extend(w.into_iter().skip(1));
                    } else {
                        w.pop();
                        w.extend(line);
                        line = w;
                    }
                } else if tail {
                    w.reverse();
                    line.extend(w.into_iter().skip(1));
                } else {
                    w.reverse();
                    w.pop();
                    w.extend(line);
                    line = w;
                }
            }
        }
        out.push(line);
    }
    out
}

fn chord_m(a: V3, b: V3) -> f64 {
    // great-circle distance in metres
    dot(a, b).clamp(-1.0, 1.0).acos() * 6_371_000.0
}

pub fn parse(cfg: &SystemConfig, text: &str) -> Result<SystemData, String> {
    let doc = json::parse(text)?;
    let elements = doc.get("elements").and_then(Json::as_arr).ok_or("no \"elements\" in response")?;

    // Stop nodes with their tags.
    let mut nodes: HashMap<i64, (LonLat, String)> = HashMap::new();
    for e in elements {
        if e.str_at("type") == Some("node") {
            if let (Some(id), Some(lat), Some(lon)) =
                (e.get("id").and_then(Json::as_f64), e.get("lat").and_then(Json::as_f64), e.get("lon").and_then(Json::as_f64))
            {
                let name = e.get("tags").map(latin_name).unwrap_or_default();
                nodes.insert(id as i64, ((lon, lat), name));
            }
        }
    }

    struct Acc {
        name: String,
        badge: String,
        color: [u8; 3],
        ways: Vec<Vec<LonLat>>,
        way_ids: HashSet<i64>,
    }
    let mut keys: HashMap<String, usize> = HashMap::new();
    let mut accs: Vec<Acc> = Vec::new();
    let mut stops: Vec<(i64, usize)> = Vec::new(); // (node id, line index)
    let mut stop_pos: HashMap<i64, LonLat> = HashMap::new();

    for e in elements {
        if e.str_at("type") != Some("relation") {
            continue;
        }
        let Some(tags) = e.get("tags") else { continue };
        let rf = tags.str_at("ref").unwrap_or("");
        let name_raw = latin_name(tags);
        let name_any = tags.str_at("name").unwrap_or("");
        let colour_tag = tags.str_at("colour").or_else(|| tags.str_at("color"));

        // Known (standardised) line?
        let known = cfg.known.iter().find(|k| {
            (!rf.is_empty() && rf == k.0)
                || (!k.1.is_empty() && (name_raw.to_lowercase().contains(&k.1.to_lowercase()) || name_any.to_lowercase().contains(&k.1.to_lowercase())))
        });
        let (key, color, badge) = match known {
            Some(k) => (
                format!("known:{}:{}", k.0, k.1),
                k.2,
                if k.0.len() <= 2 { k.0.to_uppercase() } else { String::new() },
            ),
            None => {
                let key = colour_tag
                    .map(|c| c.to_ascii_lowercase())
                    .or_else(|| (!rf.is_empty()).then(|| rf.to_string()))
                    .unwrap_or_else(|| name_any.to_string());
                let color = colour_tag.and_then(parse_color).unwrap_or_else(|| hash_color(&key));
                let badge = if !rf.is_empty() && rf.chars().count() <= 2 { rf.to_uppercase() } else { String::new() };
                (key, color, badge)
            }
        };
        let idx = *keys.entry(key).or_insert_with(|| {
            let mut name = name_raw.split(':').next().unwrap_or("").trim().to_string();
            if name.is_empty() {
                name = rf.to_string();
            }
            accs.push(Acc { name, badge, color, ways: Vec::new(), way_ids: HashSet::new() });
            accs.len() - 1
        });

        let Some(members) = e.get("members").and_then(Json::as_arr) else { continue };
        for m in members {
            let role = m.str_at("role").unwrap_or("");
            match m.str_at("type") {
                Some("way") if !role.starts_with("platform") && !role.starts_with("stop") => {
                    let id = m.get("ref").and_then(Json::as_f64).unwrap_or(-1.0) as i64;
                    if id >= 0 && !accs[idx].way_ids.insert(id) {
                        continue; // already drawn for this line (other direction / variant)
                    }
                    let pts: Vec<LonLat> = m
                        .get("geometry")
                        .and_then(Json::as_arr)
                        .map(|g| {
                            g.iter()
                                .filter_map(|p| Some((p.get("lon")?.as_f64()?, p.get("lat")?.as_f64()?)))
                                .collect()
                        })
                        .unwrap_or_default();
                    if pts.len() >= 2 {
                        accs[idx].ways.push(pts);
                    }
                }
                Some("node") if role.starts_with("stop") => {
                    if let Some(id) = m.get("ref").and_then(Json::as_f64) {
                        let id = id as i64;
                        stops.push((id, idx));
                        if let (Some(lat), Some(lon)) = (m.get("lat").and_then(Json::as_f64), m.get("lon").and_then(Json::as_f64)) {
                            stop_pos.insert(id, (lon, lat));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let to_v3 = |p: LonLat| lonlat_to_vec(p.0.to_radians(), p.1.to_radians());
    let mut data = SystemData::default();
    for a in accs {
        let paths: Vec<Vec<V3>> =
            chain(a.ways).into_iter().map(|w| w.into_iter().map(to_v3).collect()).collect();
        data.lines.push(TLine { name: a.name, badge: a.badge, color: a.color, paths, track: Track::default() });
    }

    // Stations: merge platform pairs / duplicate stops of the same name that sit close together.
    for (id, line) in stops {
        let Some(p) = nodes.get(&id).map(|n| n.0).or_else(|| stop_pos.get(&id).copied()) else { continue };
        let name = nodes.get(&id).map(|n| n.1.clone()).unwrap_or_default();
        let pos = to_v3(p);
        let radius = if name.is_empty() { 40.0 } else { 300.0 };
        let hit = data
            .stations
            .iter_mut()
            .find(|s| s.name == name && chord_m(s.pos, pos) < radius);
        match hit {
            Some(s) => {
                if !s.lines.contains(&line) {
                    s.lines.push(line);
                }
            }
            None => data.stations.push(Station { name, pos, lines: vec![line] }),
        }
    }
    data.finish();
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_ids_are_unique_and_sane() {
        for (i, c) in SYSTEMS.iter().enumerate() {
            assert!(!SYSTEMS[..i].iter().any(|o| o.id == c.id), "duplicate id {}", c.id);
            assert!(c.id.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit()), "{}", c.id);
            let [s, w, n, e] = c.bbox;
            assert!(s < n && w < e && s >= -90.0 && n <= 90.0 && w >= -180.0 && e <= 180.0, "{} bbox", c.id);
            let (lat, lon) = c.home;
            assert!(lat >= s - 0.3 && lat <= n + 0.3 && lon >= w - 0.3 && lon <= e + 0.3, "{} home outside its box", c.id);
            assert!(!c.filters.is_empty(), "{}", c.id);
        }
        assert!(SYSTEMS.len() <= 100, "{}", SYSTEMS.len());
    }

    #[test]
    fn chain_joins_ways_in_any_orientation() {
        let ways = vec![
            vec![(0.0, 0.0), (1.0, 0.0)],
            vec![(2.0, 0.0), (1.0, 0.0)], // reversed
            vec![(2.0, 0.0), (3.0, 1.0)],
            vec![(10.0, 10.0), (11.0, 10.0)], // separate piece
            vec![(-1.0, 0.0), (0.0, 0.0)],    // joins at the head
        ];
        let mut out = chain(ways);
        out.sort_by_key(|l| l.len());
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].len(), 2);
        let long = &out[1];
        assert_eq!(long.len(), 5);
        let xs: Vec<f64> = long.iter().map(|p| p.0).collect();
        assert!(xs == [-1.0, 0.0, 1.0, 2.0, 3.0] || xs == [3.0, 2.0, 1.0, 0.0, -1.0], "{xs:?}");
    }

    #[test]
    fn compact_format_round_trips() {
        let d = parse(&SYSTEMS[2], SAMPLE).unwrap();
        let bytes = encode(&d);
        let back = decode(&bytes).unwrap();
        assert_eq!(back.lines.len(), d.lines.len());
        assert_eq!(back.stations.len(), d.stations.len());
        assert_eq!(back.lines[0].badge, "G");
        assert_eq!(back.lines[0].color, [255, 149, 0]);
        assert_eq!(back.lines[0].paths[0].len(), 3);
        let (a, b) = (back.lines[0].paths[0][1], d.lines[0].paths[0][1]);
        let err = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt() * 6_371_000.0;
        assert!(err < 0.05, "{err} m");
        let names: Vec<&str> = back.stations.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Shibuya"));
        assert!(back.radius > 0.0);
        assert!(decode(&bytes[..bytes.len() - 3]).is_err());
        assert!(decode(b"nope").is_err());
    }

    #[test]
    fn known_colours_are_keyed_by_ref_so_ny_lines_stay_apart() {
        let ny = SYSTEMS.iter().find(|s| s.id == "newyork").unwrap();
        let text = r##"{"elements":[
          {"type":"relation","id":1,"tags":{"route":"subway","ref":"A","name":"A Train"},"members":[{"type":"way","ref":1,"role":"","geometry":[{"lat":40.7,"lon":-74.0},{"lat":40.8,"lon":-73.9}]}]},
          {"type":"relation","id":2,"tags":{"route":"subway","ref":"1","name":"1 Train"},"members":[{"type":"way","ref":2,"role":"","geometry":[{"lat":40.7,"lon":-74.0},{"lat":40.9,"lon":-73.9}]}]}
        ]}"##;
        let d = parse(ny, text).unwrap();
        assert_eq!(d.lines.len(), 2);
        assert_ne!(d.lines[0].color, d.lines[1].color);
    }

    #[test]
    fn every_system_is_well_formed() {
        let mut ids = std::collections::HashSet::new();
        for c in SYSTEMS {
            assert!(ids.insert(c.id), "duplicate id {}", c.id);
            let [s, w, n, e] = c.bbox;
            assert!(s < n && w < e && (-90.0..=90.0).contains(&s) && (-180.0..=180.0).contains(&w), "{}", c.id);
            assert!(c.home.0 >= s && c.home.0 <= n && c.home.1 >= w && c.home.1 <= e, "{} home outside its box", c.id);
            assert!(!c.filters.is_empty());
        }
        assert!(SYSTEMS.len() >= 25);
    }

    #[test]
    fn query_mentions_bbox_and_filters() {
        let q = overpass_query(&SYSTEMS[2]);
        assert!(q.contains("35.55,139.5,35.86,139.95"), "{q}");
        assert!(q.contains("Tokyo Metro"));
        assert!(q.contains("out geom"));
        let e = url_encode(&q);
        assert!(!e.contains(' ') && !e.contains('"') && e.contains("%5B"));
    }

    const SAMPLE: &str = r##"{"elements":[
      {"type":"relation","id":1,"tags":{"route":"subway","ref":"G","name":"Tokyo Metro Ginza Line: Shibuya => Asakusa","colour":"#ff9500"},
       "members":[
         {"type":"way","ref":100,"role":"","geometry":[{"lat":35.0,"lon":139.0},{"lat":35.0,"lon":139.1}]},
         {"type":"way","ref":101,"role":"","geometry":[{"lat":35.0,"lon":139.1},{"lat":35.1,"lon":139.1}]},
         {"type":"node","ref":7,"role":"stop","lat":35.0,"lon":139.0},
         {"type":"node","ref":8,"role":"stop","lat":35.1,"lon":139.1}]},
      {"type":"relation","id":2,"tags":{"route":"subway","ref":"G","name":"Tokyo Metro Ginza Line: Asakusa => Shibuya"},
       "members":[
         {"type":"way","ref":101,"role":"","geometry":[{"lat":35.0,"lon":139.1},{"lat":35.1,"lon":139.1}]},
         {"type":"node","ref":9,"role":"stop","lat":35.0001,"lon":139.0001}]},
      {"type":"relation","id":3,"tags":{"route":"subway","ref":"X","name":"Mystery","colour":"purple"},
       "members":[{"type":"way","ref":300,"role":"","geometry":[{"lat":35.0,"lon":139.0},{"lat":35.0,"lon":139.05}]},
                  {"type":"node","ref":7,"role":"stop","lat":35.0,"lon":139.0}]},
      {"type":"node","id":7,"lat":35.0,"lon":139.0,"tags":{"name":"渋谷","name:en":"Shibuya"}},
      {"type":"node","id":8,"lat":35.1,"lon":139.1,"tags":{"name":"浅草"}},
      {"type":"node","id":9,"lat":35.0001,"lon":139.0001,"tags":{"name:en":"Shibuya"}}
    ]}"##;

    #[test]
    fn parses_lines_dedupes_ways_and_merges_stations() {
        let d = parse(&SYSTEMS[2], SAMPLE).unwrap();
        assert_eq!(d.lines.len(), 2, "Ginza (both directions merged) + Mystery");
        let ginza = &d.lines[0];
        assert_eq!(ginza.color, [255, 149, 0], "standardised colour wins");
        assert_eq!(ginza.badge, "G");
        assert_eq!(ginza.paths.len(), 1, "two ways chained into one polyline");
        assert_eq!(ginza.paths[0].len(), 3);
        assert_eq!(d.lines[1].color, [140, 70, 160]);
        // Shibuya: nodes 7 and 9 merged, used by Ginza and Mystery => interchange
        let shibuya = d.stations.iter().find(|s| s.name == "Shibuya").unwrap();
        assert_eq!(shibuya.lines.len(), 2);
        assert_eq!(d.stations.iter().filter(|s| s.name == "Shibuya").count(), 1);
        // Asakusa has only a Japanese name: kept as an unnamed dot
        assert!(d.stations.iter().any(|s| s.name.is_empty() && s.lines == vec![0]));
    }

    #[test]
    fn trains_stop_at_stations_and_drive_at_metro_speeds() {
        let d = parse(&SYSTEMS[2], SAMPLE).unwrap();
        let t = &d.lines[0].track;
        assert!(t.stops.len() >= 2);
        assert!(t.cycle_seconds() > 0.0);
        let (mut waited, mut max_v) = (0u32, 0.0f64);
        let mut prev = t.s_at(0.0);
        for k in 1..(t.cycle_seconds() as u32 * 2) {
            let s = t.s_at(k as f64);
            let v = (s - prev).abs() * EARTH_RADIUS_M; // metres in the last second
            if v < 1e-6 {
                waited += 1;
            }
            max_v = max_v.max(v);
            prev = s;
        }
        assert!(waited >= 20, "trains must wait at stations ({waited} s)");
        assert!(max_v <= TRAIN_VMAX + 0.5, "no faster than {TRAIN_VMAX} m/s, saw {max_v}");
        assert!((t.s_at(0.0) - t.s_at(t.cycle_seconds())).abs() < 1e-9, "the cycle repeats");
    }

    #[test]
    fn drive_profile_is_smooth_and_complete() {
        for d in [150.0, 400.0, 900.0, 5000.0] {
            let total = drive_time(d);
            assert!((drive_fraction(d, total, 0.0)).abs() < 1e-9);
            assert!((drive_fraction(d, total, total) - 1.0).abs() < 1e-9);
            let mut prev = 0.0;
            for k in 0..=100 {
                let f = drive_fraction(d, total, total * k as f64 / 100.0);
                assert!(f >= prev - 1e-12);
                prev = f;
            }
        }
    }

    #[test]
    fn trains_can_be_placed_along_a_line() {
        let d = parse(&SYSTEMS[2], SAMPLE).unwrap();
        let line = &d.lines[0];
        let len = line.track.length();
        assert!(len > 0.0);
        let a = line.track.point_at(&line.paths, 0.0).unwrap();
        let b = line.track.point_at(&line.paths, len).unwrap();
        let mid = line.track.point_at(&line.paths, len * 0.5).unwrap();
        assert!(dot(a, mid) < 1.0 && dot(mid, b) < 1.0);
        assert!(d.radius > 0.0, "a freshly parsed system must be ready to draw");
    }
}
