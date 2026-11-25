# RustyMaps

A small proof of concept: a desktop app that opens on a 3D globe and zooms smoothly down to a flat street map.
Rust + [egui](https://github.com/emilk/egui) + [wgpu](https://wgpu.rs). Map data is **vector tiles from
[OpenFreeMap](https://openfreemap.org)** (no raster tiles); the map is drawn by a custom wgpu shader and the labels
(countries, states, cities, …) are drawn by egui.

## Controls

| Action | Effect |
| --- | --- |
| Left-drag | Move (pan) the map |
| Right-drag or middle-drag | Up/down: tilt (3D perspective). Left/right: rotate around the point in the middle |
| Mouse wheel / pinch | Zoom toward the cursor (down to street level, about zoom 19) |
| Double-click | Fly in on that spot |
| **R**, **Reset view**, or click the compass | North up, looking straight down |
| **Tilt** / **Rotate** sliders | The same as dragging, with numbers |
| **Go to** menu | Fly to any city. Long trips zoom out towards a globe view and zoom back in near the city |
| **Globe** button | Fly back out to the globe |
| **Clear cache** button | Delete the on-disk tile cache |

The **compass** (bottom right, can be switched off under *View*) points to north and shows the tilt.

The control window (top left, scrolls, sections collapse) also has **Map style & labels**, the **Transit maps** switches
and the **Flights** controls. It shows frame rate, zoom level, tile counts and which GPU is in use. The app always asks
for the discrete / high-performance GPU first.

## Map styles and labels

16 styles: OSM Standard, OSM Bright, OpenFreeMap Liberty, Positron (light), Dark Matter, Voyager, OSM Humanitarian,
OpenTopoMap, CyclOSM, OSM Transport, Stamen Terrain, Stamen Toner, Monochrome, Vintage, Midnight and Blueprint. These are
**colour recreations** of those looks on the one vector data set the app uses (OpenFreeMap). The real styles also differ in
which features they draw, symbols, hillshading and so on, which a recreation cannot copy; dark styles switch to light
labels. Labels can be hidden, set to Small / Medium / Large / X-large, and made bolder (no bold font is bundled, so bold
is drawn by thickening the strokes).

## Offline data folder

Aircraft models and transit maps are plain files, not downloaded or rebuilt with the program:

```
data/
  models/b737.bin  a380.bin  b787.bin     baked aircraft (assets/models in this repo)
  transit/<system>.tmap                   compact transit maps (assets/transit in this repo)
```

The program looks for it in `%RUSTYMAPS_DATA%` (environment variable), then a `data` folder next to `rustymaps.exe`, then
`%LOCALAPPDATA%\rustymaps\data` (macOS `~/Library/Application Support/rustymaps/data`, Linux `~/.local/share/rustymaps/data`).
Get the folder once from the **data** workflow (Actions, *data*, *Run workflow*; it also downloads any transit maps that are
missing and commits them to `assets/transit`, so each system is only ever downloaded once) and keep it; new builds of the
program do not need it again. A transit system without a saved file is downloaded when you switch it on, and saved.

## Transit maps

27 systems: SF Muni Metro, Los Angeles Metro, Tokyo Metro, Tokyo Toei, London Underground & DLR, New York City Subway, Paris
Métro, Moscow, Beijing, Shanghai, Guangzhou, Shenzhen, Chengdu, Wuhan, Nanjing, Hangzhou, Osaka, Seoul, Delhi, Hong Kong
MTR, Singapore MRT & LRT, Mexico City, Madrid, Berlin U-Bahn, Cairo and São Paulo. Lines follow the real tracks (route
relations from OpenStreetMap) but are drawn like a subway map: thick colour-coded lines with a white outline and lettered
roundels at the line ends, with official colours where they are known (London, New York, Paris, Tokyo, LA, Muni) and the
colour in the OpenStreetMap data elsewhere. Zoomed out, each city whose map is saved in the data folder (those are switched on at start-up) shows a blue **metro symbol**; as you zoom in to city level the symbol fades out while the slim lines fade in, and they get thicker the closer you get. **Station circles and
names only appear once there is room for them**: a circle needs about 18 px to its nearest neighbour, a name about 56 px.

Systems are plain entries in `src/transit_data.rs` (bounding box, OSM filters, standard line colours), so adding another
city is one more entry. `rustymaps.exe --fetch-transit <folder> [system ids]` downloads maps by hand.

## Flight simulation

Three aircraft shuttle between their airports until you pause them: a Southwest 737 between Las Vegas and Lubbock, a
Lufthansa A380 between San Francisco and Frankfurt, and an ANA 787 Dreamliner between San Francisco and Tokyo Haneda. Each
has its own play/pause, progress slider, **Look at** and **Follow**. At the far end an aircraft waits about six seconds,
turns around and flies back. Shared sliders set the speed (real time to 3600x), the **model size** (1x to 20,000x) and the
**altitude exaggeration** (1x to 150x; the true height is shown in feet). Route lines are blue and semi-transparent by default; colour, opacity and width are adjustable. A drop line and a dot mark the spot on the ground
below each aircraft, and the aircraft are always drawn above the route lines and station markers.

## How it works

- Tiles (`z/x/y.pbf`, OpenMapTiles schema) are downloaded by 4 worker threads, decoded by a small built-in Mapbox
  Vector Tile reader, triangulated (`earcutr`) and cut into pieces of at most ~3° so they bend with the globe.
- The GPU maps every vertex from Web Mercator onto the sphere in the vertex shader, so zooming and rotating never
  re-uploads geometry. Positions are computed relative to the point under the camera with cancellation-free
  formulas, so they stay precise at any zoom. Zoomed in far enough, the sphere is indistinguishable from a flat map.
- Roads/borders are drawn as constant-pixel-width anti-aliased lines; widths and visibility follow the zoom level.
- While a tile is still loading, its nearest loaded parent tile is shown instead.
- Labels are projected from the tiles' point features and placed by priority with simple collision avoidance.

## Cache and politeness

Tiles are cached in `%LOCALAPPDATA%\rustymaps\tiles` (Windows), `~/Library/Caches/rustymaps/tiles` (macOS) or
`~/.cache/rustymaps/tiles` (Linux). The cache is capped at **2 GB** (oldest files are removed first) and entries
older than 30 days are fetched again. At most 4 requests run at once, each with an identifying `User-Agent`, and failed
tiles back off (8 s, 16 s, 32 s, 60 s) instead of being retried in a loop.

## Known limits

- Maximum zoom is about level 19. The data itself stops at level 14, so beyond that you are magnifying the level-14 geometry (roads and outlines stay crisp, but there is no extra detail).
- Map styles are colour recreations, not the original style sheets.
- Where several transit lines share a track they are drawn on top of each other rather than side by side.
- Transit route data comes from OpenStreetMap tags that I could not test against the live service from my sandbox; if a system fails to load, the control window says why.
- While zooming quickly, areas with a loaded parent but loading children may briefly show doubled road lines.
- Labels are horizontal and do not follow curves.

## Build

```
cargo run --release
```

GitHub Actions (`.github/workflows/build.yml`) builds a Windows release on every push and uploads
`rustymaps<YYMMDDHHMM>.zip` (just the program) as an artifact; `.github/workflows/data.yml` (run by hand) builds
`rustymaps-data<YYMMDDHHMM>.zip` with the data folder. Aircraft are baked from Sketchfab glTF files with `tools/bake_plane.py`. Linux and macOS use the same code; they just are not in CI yet.
`Cargo.lock` is not committed yet; the first CI build creates it.

## Credits

Map data © [OpenStreetMap](https://www.openstreetmap.org/copyright) contributors, tiles by [OpenFreeMap](https://openfreemap.org)
(© [OpenMapTiles](https://openmaptiles.org/)). Aircraft models from Sketchfab, all CC-BY-4.0:
[Southwest Airlines Boeing 737](https://sketchfab.com/3d-models/southwest-airlines-boeing-737-73d0cd0c03594c5d9c53235d74b429bf) by A Random Modeler,
[Airbus A380-841 Lufthansa](https://sketchfab.com/3d-models/airbus-a380-841-lufthansa-edbffac71f88480c82f5690b777e7416) by Nobilis 2,
[Boeing-787- Dreamliner](https://sketchfab.com/3d-models/boeing-787-dreamliner-524f485c748146b7968e0dd8eb84e786) by themaker845
(see `assets/models/CREDITS.txt`; the baked files are adaptations).

Please keep the on-screen attribution in place.
