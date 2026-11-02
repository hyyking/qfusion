// One draw per shader stage. Stage state (tcmod matrix, rgbgen/alphagen, alphafunc) is
// evaluated on the CPU each frame and passed in the dynamic-offset `stage` uniform.

struct View {
    mvp: mat4x4<f32>,
    // xyz: eye position, w: shader time in seconds.
    eye: vec4<f32>,
    // x: lightmap/vertex light scale, y: dynamic light count.
    params: vec4<f32>,
    // Per dynamic light: xyz origin + w radius, then rgb colour.
    lights: array<vec4<f32>, 8>,
};

fn dynamic_light(pos: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    var sum = vec3<f32>(0.0);
    for (var i = 0u; i < u32(view.params.y); i++) {
        let to_light = view.lights[i * 2u].xyz - pos;
        let dist = length(to_light);
        let atten = clamp(1.0 - dist / view.lights[i * 2u].w, 0.0, 1.0);
        let facing = clamp(dot(normalize(normal), to_light / max(dist, 1.0)), 0.0, 1.0);
        sum += view.lights[i * 2u + 1u].rgb * atten * (0.3 + 0.7 * facing);
    }
    return sum;
}

struct Stage {
    tc0: vec4<f32>,
    tc1: vec4<f32>,
    vec_s: vec4<f32>,
    vec_t: vec4<f32>,
    color: vec4<f32>,
    // x: map kind (0 texture, 1 lightmap, 2 white), y: rgb mode (0 const, 1 vertex,
    // 2 one-minus-vertex), z: alpha mode (0 const, 1 vertex), w: tcgen (0 base,
    // 1 lightmap, 2 environment, 3 vector).
    modes: vec4<u32>,
    // x: alpha func (0 none, 1 gt0, 2 lt128, 3 ge128).
    test: vec4<u32>,
    // x: turb amplitude, y: turb phase (already including time).
    turb: vec4<f32>,
};

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var lightmap: texture_2d<f32>;
@group(0) @binding(2) var lm_sampler: sampler;
@group(1) @binding(0) var<uniform> stage: Stage;
@group(1) @binding(1) var tex: texture_2d<f32>;
@group(1) @binding(2) var tex_sampler: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) lm_uv: vec2<f32>,
    @location(4) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) lm_uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) world_pos: vec3<f32>,
    @location(4) world_normal: vec3<f32>,
};

const TAU: f32 = 6.2831853;

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.pos = view.mvp * vec4<f32>(in.position, 1.0);
    var uv = in.uv;
    switch stage.modes.w {
        case 1u: {
            uv = in.lm_uv;
        }
        case 2u: {
            let v = normalize(in.position - view.eye.xyz);
            let r = v - 2.0 * dot(in.normal, v) * in.normal;
            uv = vec2<f32>(0.5 + r.y * 0.5, 0.5 - r.z * 0.5);
        }
        case 3u: {
            uv = vec2<f32>(dot(in.position, stage.vec_s.xyz), dot(in.position, stage.vec_t.xyz));
        }
        default: {}
    }
    if stage.turb.x != 0.0 {
        let p = in.position * (0.125 / 128.0);
        uv += stage.turb.x * vec2<f32>(
            sin((p.x + p.z + stage.turb.y) * TAU),
            sin((p.y + stage.turb.y) * TAU),
        );
    }
    out.uv = vec2<f32>(
        stage.tc0.x * uv.x + stage.tc0.y * uv.y + stage.tc0.z,
        stage.tc1.x * uv.x + stage.tc1.y * uv.y + stage.tc1.z,
    );
    out.lm_uv = in.lm_uv;
    out.color = in.color;
    out.world_pos = in.position;
    out.world_normal = in.normal;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(tex, tex_sampler, in.uv);
    let lit = textureSampleLevel(lightmap, lm_sampler, in.lm_uv, 0.0).rgb;
    var base = vec4<f32>(1.0);
    switch stage.modes.x {
        case 0u: {
            base = texel;
        }
        case 1u: {
            // Surfaces without a lightmap fall back to their vertex colours.
            var light = lit;
            if in.lm_uv.x < 0.0 {
                light = in.color.rgb;
            }
            base = vec4<f32>(
                light * view.params.x + dynamic_light(in.world_pos, in.world_normal),
                1.0,
            );
        }
        default: {}
    }
    var rgb = stage.color.rgb;
    if stage.modes.y == 1u {
        rgb = in.color.rgb;
    } else if stage.modes.y == 2u {
        rgb = vec3<f32>(1.0) - in.color.rgb;
    }
    var alpha = stage.color.a;
    if stage.modes.z == 1u {
        alpha = in.color.a;
    }
    let result = vec4<f32>(base.rgb * rgb, base.a * alpha);
    switch stage.test.x {
        case 1u: {
            if result.a <= 0.0 {
                discard;
            }
        }
        case 2u: {
            if result.a >= 0.5 {
                discard;
            }
        }
        case 3u: {
            if result.a < 0.5 {
                discard;
            }
        }
        default: {}
    }
    return result;
}
