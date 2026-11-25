# RustyMaps

A small proof of concept: a desktop app that opens on a 3D globe and zooms smoothly down to a flat street map.
Rust + [egui](https://github.com/emilk/egui) + [wgpu](https://wgpu.rs). Map data is **vector tiles from
[OpenFreeMap](https://openfreemap.org)** (no raster tiles); the map is drawn by a custom wgpu shader and the labels
(countries, states, cities, …) are drawn by egui.

## Controls

| Action | Effect |
| --- | --- |
| Drag | Pan |
| Right-drag, or the **Tilt** slider | Tilt the view (3D perspective) |
| Mouse wheel / pinch | Zoom toward the cursor (down to street level, about zoom 19) |
| Double-click | Fly in on that spot |
| **San Francisco** button | Fly from wherever you are to San Francisco (zoom ≈ 12.5) |
| **Globe** button | Fly back out to the globe |
| **Clear cache** button | Delete the on-disk tile cache |

The control window (top left, collapsible) also has the map **Style** (OSM Standard, OSM Bright, Light), the
**Transit maps** switches and the **Flight** controls described below. It shows frame rate, zoom level, tile counts and which GPU is in use. The app always asks for the
discrete / high-performance GPU first.

## Transit maps

Switch on **SF Muni Metro**, **Los Angeles Metro** or **Tokyo Metro** in the control window. Lines follow the real
tracks (route relations from OpenStreetMap) but are drawn like a subway map: thick colour-coded lines with a white
outline, white station circles with a dark ring (bigger at interchanges), lettered roundels at the line ends, and
Tokyo Metro / LA Metro / Muni line colours. They appear from zoom 8, stations from 10.5.

The route data is downloaded once from the public Overpass API (one request per system, with an identifying
`User-Agent`) and cached next to the tile cache for 30 days. Systems are plain entries in `src/transit_data.rs`
(bounding box, OSM filters, standard line colours), so adding another city is one more entry.

## Flight simulation

A Boeing 737 (the Southwest Airlines model, see credits) flies the great-circle route San Francisco → Tokyo Haneda at
its real cruise altitude (35,000 ft) in about 11 simulated hours. Use the **progress slider** to place it anywhere on
the route, **Fly** to let it move on its own (real time, 60x, 600x or 3600x), the **Plane size** slider to scale the
model from 1x to 20,000x, **Look at plane** to fly the camera to it, and **Follow** to keep it centred. Tilt the view to
see the model in 3D. At its true altitude the plane is only in front of the camera when you are higher than ~11 km, so
at street zoom it is, correctly, behind you.

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
- The view is always north-up (tilt only; no rotation).
- Where several transit lines share a track they are drawn on top of each other rather than side by side.
- Transit route data comes from OpenStreetMap tags that I could not test against the live service from my sandbox; if a system fails to load, the control window says why.
- While zooming quickly, areas with a loaded parent but loading children may briefly show doubled road lines.
- Labels are horizontal and do not follow curves.

## Build

```
cargo run --release
```

GitHub Actions (`.github/workflows/build.yml`) builds a Windows release on every push and uploads
`rustymaps<YYMMDDHHMM>.zip` as an artifact. Linux and macOS use the same code; they just are not in CI yet.
`Cargo.lock` is not committed yet; the first CI build creates it.

## Attribution

3D model: "Southwest Airlines Boeing 737" by A Random Modeler, <https://sketchfab.com/3d-models/southwest-airlines-boeing-737-73d0cd0c03594c5d9c53235d74b429bf>,
licensed CC-BY-4.0 (<http://creativecommons.org/licenses/by/4.0/>). It was converted to the compact `assets/plane.bin`
with `tools/bake_plane.py`.

Map data © [OpenStreetMap contributors](https://www.openstreetmap.org/copyright), schema © [OpenMapTiles](https://openmaptiles.org/),
tiles served by [OpenFreeMap](https://openfreemap.org). Please keep the on-screen attribution in place.
