# RustyMaps

A small proof of concept: a desktop app that opens on a 3D globe and zooms smoothly down to a flat street map.
Rust + [egui](https://github.com/emilk/egui) + [wgpu](https://wgpu.rs). Map data is **vector tiles from
[OpenFreeMap](https://openfreemap.org)** (no raster tiles); the map is drawn by a custom wgpu shader and the labels
(countries, states, cities, …) are drawn by egui.

## Controls

| Action | Effect |
| --- | --- |
| Drag | Pan |
| Mouse wheel / pinch | Zoom toward the cursor |
| Double-click | Fly in on that spot |
| **San Francisco** button | Fly from wherever you are to San Francisco (zoom ≈ 12.5) |
| **Globe** button | Fly back out to the globe |
| **Clear cache** button | Delete the on-disk tile cache |

The corner readout shows frame rate, zoom level, tile counts and which GPU is in use. The app always asks for the
discrete / high-performance GPU first.

## How it works

- Tiles (`z/x/y.pbf`, OpenMapTiles schema) are downloaded by 4 worker threads, decoded by a small built-in Mapbox
  Vector Tile reader, triangulated (`earcutr`) and cut into pieces of at most ~3° so they bend with the globe.
- The GPU maps every vertex from Web Mercator onto the sphere in the vertex shader, so zooming and rotating never
  re-uploads geometry. Zoomed in far enough, the sphere is indistinguishable from a flat map.
- Roads/borders are drawn as constant-pixel-width anti-aliased lines; widths and visibility follow the zoom level.
- While a tile is still loading, its nearest loaded parent tile is shown instead.
- Labels are projected from the tiles' point features and placed by priority with simple collision avoidance.

## Cache and politeness

Tiles are cached in `%LOCALAPPDATA%\rustymaps\tiles` (Windows), `~/Library/Caches/rustymaps/tiles` (macOS) or
`~/.cache/rustymaps/tiles` (Linux). The cache is capped at **2 GB** (oldest files are removed first) and entries
older than 30 days are fetched again. At most 4 requests run at once, each with an identifying `User-Agent`, and failed
tiles back off (8 s, 16 s, 32 s, 60 s) instead of being retried in a loop.

## Known limits

- Maximum zoom is about level 13.7: the GPU works in 32-bit floats, and deeper zoom would make geometry jitter.
- The view is north-up (no tilt or rotation).
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

Map data © [OpenStreetMap contributors](https://www.openstreetmap.org/copyright), schema © [OpenMapTiles](https://openmaptiles.org/),
tiles served by [OpenFreeMap](https://openfreemap.org). Please keep the on-screen attribution in place.
