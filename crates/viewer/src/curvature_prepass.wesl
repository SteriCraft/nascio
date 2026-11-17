// Prepass and shadow-pass vertex shader of the curved materials: the same drop as the main
// pass (`curvature.wesl`), measured from the camera given in the uniform (in the shadow pass
// the view is the light), so shadows fall on the displayed, lowered geometry.

import bevy_pbr::{
    prepass::io::{Vertex, VertexOutput, decompress_vertex},
    render::{
        mesh_functions,
        view_transformations::position_world_to_clip,
    },
};

struct Curved {
    drop: vec4<f32>,
    sun: vec4<f32>,
    seam: vec4<f32>,
    light: vec4<f32>,
    snow: vec4<f32>,
    air: vec4<f32>,
    haze: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<storage, read> curved: Curved;

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let v = decompress_vertex(vertex_in, vertex_in.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(vertex_in.instance_index);

    var world = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(v.position, 1.0));
    let d = world.xz - curved.drop.yw;
    world.y -= curved.drop.x * dot(d, d);
    out.world_position = world;
    out.position = position_world_to_clip(world.xyz);
@if(UNCLIPPED_DEPTH_ORTHO_EMULATION) {
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
}

@if(VERTEX_UVS_A)
    out.uv = v.uv;
@if(VERTEX_UVS_B)
    out.uv_b = v.uv_b;
@if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_NORMALS)
    out.world_normal = mesh_functions::mesh_normal_local_to_world(v.normal, vertex_in.instance_index);
@if(VERTEX_COLORS)
    out.color = v.color;
@if(MOTION_VECTOR_PREPASS)
    out.previous_world_position = world;
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
@if(VISIBILITY_RANGE_DITHER)
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex_in.instance_index, world_from_local[3]);

    return out;
}
