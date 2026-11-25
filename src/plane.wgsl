// The 3D plane. It is drawn into its own (supersampled) texture with its own depth buffer,
// then that texture is laid over the map. This keeps the model's depth testing independent of
// the map pass.

struct U {
    mvp: mat4x4<f32>,
    nrm: mat4x4<f32>,    // model normals -> view space (rotation only)
    flags: vec4<f32>,    // x = 1 when the target is sRGB
};
@group(0) @binding(0) var<uniform> u: U;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) n: vec3<f32>,
    @location(1) color: vec3<f32>,
};

@vertex
fn vs_plane(@location(0) p: vec3<f32>, @location(1) n: vec4<f32>, @location(2) c: vec4<f32>) -> VOut {
    var o: VOut;
    o.pos = u.mvp * vec4<f32>(p, 1.0);
    o.n = (u.nrm * vec4<f32>(n.xyz, 0.0)).xyz;
    o.color = c.rgb;
    return o;
}

@fragment
fn fs_plane(v: VOut) -> @location(0) vec4<f32> {
    var n = normalize(v.n);
    // The model is double-sided; light whichever face points at the camera.
    if (n.z < 0.0) {
        n = -n;
    }
    let l = normalize(vec3<f32>(-0.45, 0.65, 0.75));
    let h = normalize(l + vec3<f32>(0.0, 0.0, 1.0));
    let diffuse = max(dot(n, l), 0.0);
    let spec = pow(max(dot(n, h), 0.0), 40.0) * 0.25;
    var rgb = v.color * (0.38 + 0.78 * diffuse) + vec3<f32>(spec);
    rgb = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    if (u.flags.x < 0.5) {
        rgb = pow(rgb, vec3<f32>(1.0 / 2.2));
    }
    return vec4<f32>(rgb, 1.0);
}

// ---- composite: full-screen triangle that lays the plane texture over the map ----

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct FOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> FOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var o: FOut;
    o.pos = vec4<f32>(p[i], 0.0, 1.0);
    o.uv = vec2<f32>((p[i].x + 1.0) * 0.5, (1.0 - p[i].y) * 0.5);
    return o;
}

@fragment
fn fs_full(v: FOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, v.uv);
}
