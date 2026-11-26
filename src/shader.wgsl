// Globe + vector map shader.
//
// Tile geometry is stored in tile-local units (0..4096). The vertex shader maps
// those to normalized Web Mercator and onto the unit sphere *relative to the
// point under the camera*, using formulas without cancellation, so the result
// stays precise at any zoom (absolute f32 sphere coordinates would jitter once a
// pixel is smaller than ~1e-7 planet radii). Nothing is re-uploaded when the
// camera moves; only a tiny per-tile offset changes each frame.
//
// There is no depth buffer: the far side of the planet is removed with a
// horizon test, and layers are painted back-to-front by draw order.

struct Style {
    color: vec4<f32>,
    params: vec4<f32>,   // x = line width (px), y = casing width (px), z = 1 to thin out with distance
};

struct Globals {
    view_proj: mat4x4<f32>,  // absolute world -> clip (base sphere only)
    proj: mat4x4<f32>,       // target-relative (east, north, radial) -> clip (tiles)
    cam_pos: vec4<f32>,  // xyz = camera position in planet radii, w = 1 if target is sRGB
    cam_rel: vec4<f32>,  // x = cos(target lat), y = sin(target lat), z = alt*cos(pitch), w = alt*sin(pitch)
    head: vec4<f32>,     // x = sin(heading), y = cos(heading)
    screen: vec4<f32>,   // xy = viewport size in physical pixels
    styles: array<Style, 32>,
};

struct Tile {
    // xy = tile origin minus camera, in normalized mercator (computed in f64 on the CPU),
    // z = mercator size of one tile-local unit.
    rel: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var<uniform> tile: Tile;

const PI: f32 = 3.14159265358979;

// Position of a tile-local point relative to the target point on the surface:
// x = east, y = north, z = radial offset (<= 0, equals cos(angle) - 1).
fn rel_pos(p: vec2<f32>) -> vec3<f32> {
    let dm = tile.rel.xy + p * tile.rel.z;
    let dl = dm.x * 2.0 * PI;
    let dy = -dm.y * 2.0 * PI;
    let t = tanh(dy);
    let sg = 1.0 / cosh(dy);
    let cc = g.cam_rel.x;
    let sc = g.cam_rel.y;
    let d = 1.0 + sc * t;
    let sh = sin(0.5 * dl);
    let q = t * t / (1.0 + sg) + sg * 2.0 * sh * sh;
    return vec3<f32>(cc * sg * sin(dl) / d, cc * (sc * q + t) / d, -cc * cc * q / d);
}

fn rel_clip(r: vec3<f32>) -> vec4<f32> {
    return g.proj * vec4<f32>(r, 1.0);
}

// > 0 when the point is on the visible side of the horizon.
fn rel_horizon(r: vec3<f32>) -> f32 {
    // dot(point, camera) - 1, with the camera (1 + alt*cos p) above the target and alt*sin p
    // behind it (opposite to the heading).
    return g.cam_rel.z + r.z * (1.0 + g.cam_rel.z) - g.cam_rel.w * (g.head.x * r.x + g.head.y * r.y);
}

// Premultiplied output; converts to linear when the surface format is sRGB.
fn finish(c: vec4<f32>) -> vec4<f32> {
    var rgb = c.rgb;
    if (g.cam_pos.w > 0.5) {
        rgb = pow(rgb, vec3<f32>(2.2));
    }
    return vec4<f32>(rgb * c.a, c.a);
}

// ---------------------------------------------------------------------------
// Base sphere (land colour under everything)
// ---------------------------------------------------------------------------

struct BaseOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) horizon: f32,
};

@vertex
fn vs_base(@location(0) p: vec3<f32>) -> BaseOut {
    var o: BaseOut;
    o.pos = g.view_proj * vec4<f32>(p, 1.0);
    o.horizon = dot(p, g.cam_pos.xyz) - 1.0;
    return o;
}

@fragment
fn fs_base(v: BaseOut) -> @location(0) vec4<f32> {
    // Slightly generous: the base mesh is coarse, so its chords sit a little
    // below the true surface.
    if (v.horizon < -0.004) {
        discard;
    }
    return finish(g.styles[0].color);
}

// ---------------------------------------------------------------------------
// Filled polygons
// ---------------------------------------------------------------------------

struct FillOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) horizon: f32,
};

@vertex
fn vs_fill(@location(0) p: vec2<f32>, @location(1) style: u32) -> FillOut {
    let r = rel_pos(p);
    var o: FillOut;
    o.pos = rel_clip(r);
    o.color = g.styles[style].color;
    o.horizon = rel_horizon(r);
    return o;
}

@fragment
fn fs_fill(v: FillOut) -> @location(0) vec4<f32> {
    if (v.horizon < 0.0 || v.color.a <= 0.0) {
        discard;
    }
    return finish(v.color);
}

// ---------------------------------------------------------------------------
// Lines: one instance per segment, expanded to a quad of constant pixel width
// ---------------------------------------------------------------------------

struct LineOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) horizon: f32,
    @location(2) dist: f32,     // signed distance from the centre line, px
    @location(3) half_w: f32,   // half the line width, px
    @location(4) casing: f32,   // casing width, px
};

@vertex
fn vs_line(
    @builtin(vertex_index) vi: u32,
    @location(0) ab: vec4<f32>,
    @location(1) style: u32,
) -> LineOut {
    var o: LineOut;
    let st = g.styles[style];
    let w_px = st.params.x;
    if (w_px <= 0.0 || st.color.a <= 0.0) {
        // Hidden at this zoom: park the vertex outside the clip volume.
        o.pos = vec4<f32>(0.0, 0.0, 2.0, 1.0);
        return o;
    }

    var ts = array<f32, 6>(0.0, 0.0, 1.0, 1.0, 0.0, 1.0);
    var ss = array<f32, 6>(-1.0, 1.0, -1.0, -1.0, 1.0, 1.0);
    let t = ts[vi];
    let side = ss[vi];

    let ra = rel_pos(ab.xy);
    let rb = rel_pos(ab.zw);
    let ca = rel_clip(ra);
    let cb = rel_clip(rb);

    // Work in centred pixel space (y up).
    let res = g.screen.xy;
    let pa = (ca.xy / ca.w) * 0.5 * res;
    let pb = (cb.xy / cb.w) * 0.5 * res;
    var dir = pb - pa;
    let len = length(dir);
    if (len < 1e-5) {
        dir = vec2<f32>(1.0, 0.0);
    } else {
        dir = dir / len;
    }
    let nrm = vec2<f32>(-dir.y, dir.x);

    let half_w = w_px * 0.5 * keep;
    let pad = half_w + 0.5;            // room for the antialiased edge
    let at_end = t > 0.5;
    let base = select(pa, pb, at_end);
    let clip = select(ca, cb, at_end);
    let hz = select(rel_horizon(ra), rel_horizon(rb), at_end);

    // Small roads and paths thin out and fade towards the horizon of a tilted view, where they
    // would only add noise: full strength within 1.5x the distance to the view centre, gone at 3x.
    let far = clip.w / max(length(vec2<f32>(g.cam_rel.z, g.cam_rel.w)), 1e-9);
    let keep = mix(1.0, clamp((3.0 - far) / 1.5, 0.0, 1.0), st.params.z);
    if (keep <= 0.02) {
        o.pos = vec4<f32>(0.0, 0.0, 2.0, 1.0);
        return o;
    }

    // Square caps (extend by `pad` along the line) hide gaps at joints.
    let px = base + dir * (t * 2.0 - 1.0) * pad + nrm * side * pad;
    let ndc = px / (0.5 * res);

    o.pos = vec4<f32>(ndc * clip.w, clip.z, clip.w);
    o.color = vec4<f32>(st.color.rgb, st.color.a * keep);
    o.horizon = hz;
    o.dist = side * pad;
    o.half_w = half_w;
    o.casing = st.params.y;
    return o;
}

@fragment
fn fs_line(v: LineOut) -> @location(0) vec4<f32> {
    if (v.horizon < 0.0 || v.color.a <= 0.0 || v.half_w <= 0.0) {
        discard;
    }
    let inside = v.half_w - abs(v.dist);            // >0 inside the line
    let cover = clamp(inside + 0.5, 0.0, 1.0);      // antialiased outer edge
    if (cover <= 0.0) {
        discard;
    }
    let core = clamp(inside - v.casing + 0.5, 0.0, 1.0);
    let rgb = mix(v.color.rgb * 0.62, v.color.rgb, core);
    return finish(vec4<f32>(rgb, v.color.a * cover));
}
