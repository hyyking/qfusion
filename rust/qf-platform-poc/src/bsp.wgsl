struct Uniforms {
    mvp: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) normal: vec3<f32>,
};

@vertex
fn vs_main(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>) -> VsOut {
    var out: VsOut;
    out.pos = u.mvp * vec4<f32>(position, 1.0);
    out.normal = normal;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let normal_length = dot(in.normal, in.normal);
    var normal = vec3<f32>(0.0, 0.0, 1.0);
    if normal_length > 0.000001 {
        normal = normalize(in.normal);
    }
    let light = normalize(vec3<f32>(0.4, -0.5, 0.75));
    let diffuse = max(dot(normal, light), 0.0);
    let brightness = 0.3 + diffuse * 0.7;
    return vec4<f32>(vec3<f32>(0.55, 0.64, 0.78) * brightness, 1.0);
}
