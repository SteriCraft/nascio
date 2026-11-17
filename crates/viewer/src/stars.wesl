// Stars (spec §2.8): small quads, brightness and tint in the vertex colour, added to the sky.
// Hidden below the displayed limb (the planet is in front of them from high up).

import bevy_pbr::render::{
    forward_io::VertexOutput,
    mesh_view_bindings::view,
};
@if(TONEMAP_IN_SHADER) import bevy_core_pipeline::tonemapping::tone_mapping;

struct Stars {
    // x: visibility (night, daylight thinned by height, clouds); y: camera height (m);
    // z: k = 1 / (2 R_vis), 0 when flat; w: unused.
    u: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> stars: Stars;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let d = normalize(in.world_position.xyz - view.world_position.xyz);
    let h = max(stars.u.y, 0.0);
    // Below the limb of the lowered ground y = −k·d², seen from height h: slope < −2√(k h).
    let t = d.y / max(length(d.xz), 1e-5);
    let limb = select(0.0, -2.0 * sqrt(stars.u.z * h), stars.u.z > 0.0);
    if t < limb {
        discard;
    }
    var c = vec3<f32>(1.0);
@if(VERTEX_COLORS)
    c = in.color.rgb;
    // Round points: a soft radial profile over the quad (uv from -1 to 1), not squares.
    var r2 = 0.0;
@if(VERTEX_UVS_A)
    r2 = dot(in.uv, in.uv);
    if r2 >= 1.0 {
        discard;
    }
    c *= 2.2 * exp(-5.0 * r2) * (1.0 - smoothstep(0.6, 1.0, r2));
    // Additive (premultiplied blending with a zero alpha), tone mapped like the sky.
    var out = vec4<f32>(c * stars.u.x, 0.0);
@if(TONEMAP_IN_SHADER) {
    out = tone_mapping(out, view.color_grading);
}
    return out;
}
