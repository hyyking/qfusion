// Static md3 props: one uniform per placement (dynamic offset), one texture per surface.

struct View {
    mvp: mat4x4<f32>,
    eye: vec4<f32>,
    // x: dynamic light count.
    light_count: vec4<f32>,
    // Per dynamic light: xyz origin + w radius, then rgb colour.
    lights: array<vec4<f32>, 8>,
};

struct Instance {
    model: mat4x4<f32>,
    tint: vec4<f32>,
    light_dir: vec4<f32>,
    ambient: vec4<f32>,
    diffuse: vec4<f32>,
};

@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var<uniform> inst: Instance;
@group(2) @binding(0) var tex: texture_2d<f32>;
@group(2) @binding(1) var tex_sampler: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) world_pos: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = inst.model * vec4<f32>(in.position, 1.0);
    out.pos = view.mvp * world;
    out.world_pos = world.xyz;
    out.uv = in.uv;
    out.normal = (inst.model * vec4<f32>(in.normal, 0.0)).xyz;
    return out;
}

fn dynamic_light(pos: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    var sum = vec3<f32>(0.0);
    for (var i = 0u; i < u32(view.light_count.x); i++) {
        let to_light = view.lights[i * 2u].xyz - pos;
        let dist = length(to_light);
        let atten = clamp(1.0 - dist / view.lights[i * 2u].w, 0.0, 1.0);
        let facing = clamp(dot(normal, to_light / max(dist, 1.0)), 0.0, 1.0);
        sum += view.lights[i * 2u + 1u].rgb * atten * (0.3 + 0.7 * facing);
    }
    return sum;
}

fn shade(in: VsOut) -> vec4<f32> {
    let texel = textureSample(tex, tex_sampler, in.uv);
    let normal = normalize(in.normal);
    let facing = max(dot(normal, inst.light_dir.xyz), 0.0);
    let light = inst.ambient.rgb + inst.diffuse.rgb * facing + dynamic_light(in.world_pos, normal);
    return vec4<f32>(texel.rgb * inst.tint.rgb * light, texel.a * inst.tint.a);
}

@fragment
fn fs_opaque(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(shade(in).rgb, 1.0);
}

@fragment
fn fs_test(in: VsOut) -> @location(0) vec4<f32> {
    let c = shade(in);
    if c.a < 0.5 {
        discard;
    }
    return c;
}

@fragment
fn fs_blend(in: VsOut) -> @location(0) vec4<f32> {
    return shade(in);
}
