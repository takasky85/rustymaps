// Globe + vector map shader.
//
// Tile geometry is stored in tile-local units (0..4096). The vertex shader
// maps those to normalized Web Mercator, then onto the unit sphere, then
// through the camera. Nothing is ever re-uploaded when the camera moves.
//
// There is no depth buffer: the far side of the planet is removed with a
// horizon test, and layers are painted back-to-front by draw order.

struct Style {
    color: vec4<f32>,
    params: vec4<f32>,   // x = line width (px), y = casing width (px)
};

struct Globals {
    view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,  // xyz = camera position in planet radii, w = 1 if target is sRGB
    screen: vec4<f32>,   // xy = viewport size in physical pixels
    styles: array<Style, 32>,
};

struct Tile {
    os: vec4<f32>,       // xy = tile origin (mercator 0..1), z = tile size, w = extent
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var<uniform> tile: Tile;

const PI: f32 = 3.14159265358979;

fn tile_to_merc(p: vec2<f32>) -> vec2<f32> {
    return tile.os.xy + p / tile.os.w * tile.os.z;
}

fn merc_to_sphere(m: vec2<f32>) -> vec3<f32> {
    let lon = (m.x - 0.5) * 2.0 * PI;
    let lat = atan(sinh(PI * (1.0 - 2.0 * m.y)));
    let cl = cos(lat);
    return vec3<f32>(cl * sin(lon), sin(lat), cl * cos(lon));
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
    let s = merc_to_sphere(tile_to_merc(p));
    var o: FillOut;
    o.pos = g.view_proj * vec4<f32>(s, 1.0);
    o.color = g.styles[style].color;
    o.horizon = dot(s, g.cam_pos.xyz) - 1.0;
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

    let sa = merc_to_sphere(tile_to_merc(ab.xy));
    let sb = merc_to_sphere(tile_to_merc(ab.zw));
    let ca = g.view_proj * vec4<f32>(sa, 1.0);
    let cb = g.view_proj * vec4<f32>(sb, 1.0);

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

    let half_w = w_px * 0.5;
    let pad = half_w + 0.5;            // room for the antialiased edge
    let at_end = t > 0.5;
    let base = select(pa, pb, at_end);
    let clip = select(ca, cb, at_end);
    let sph = select(sa, sb, at_end);

    // Square caps (extend by `pad` along the line) hide gaps at joints.
    let px = base + dir * (t * 2.0 - 1.0) * pad + nrm * side * pad;
    let ndc = px / (0.5 * res);

    o.pos = vec4<f32>(ndc * clip.w, clip.z, clip.w);
    o.color = st.color;
    o.horizon = dot(sph, g.cam_pos.xyz) - 1.0;
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
