// Lays the cached map image over the map rectangle (the viewport is set to that rectangle).
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct FOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_blit(@builtin(vertex_index) i: u32) -> FOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var o: FOut;
    o.pos = vec4<f32>(p[i], 0.0, 1.0);
    o.uv = vec2<f32>((p[i].x + 1.0) * 0.5, (1.0 - p[i].y) * 0.5);
    return o;
}

@fragment
fn fs_blit(v: FOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, v.uv);
}
