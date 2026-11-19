// Prepass and shadow-pass shaders of the curved materials.
//
// Vertex: the same drop as the main pass (`curvature.wesl`), measured from the camera given in
// the uniform (in the shadow pass the view is the light), so shadows fall on the displayed,
// lowered geometry.
//
// Fragment (run for the alpha-mask foliage material, or when depth clipping is emulated): the
// foliage is cut out by its texture's alpha as in the main pass, so its shadows are cut out too.

import bevy_pbr::{
    prepass::io::{Vertex, VertexOutput, decompress_vertex},
    render::{
        mesh_functions,
        view_transformations::position_world_to_clip,
    },
};

@if(PREPASS_FRAGMENT)
import bevy_pbr::prepass::io::FragmentOutput;

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
@group(constants::MATERIAL_BIND_GROUP) @binding(105) var block_tex: texture_2d_array<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(106) var block_smp: sampler;

// Same constants as `curvature.wesl` (checked by a test).
const MATERIAL_FOLIAGE_CUT: f32 = 3.0;
const FOLIAGE_CUTOFF: f32 = 0.5;

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

// Discards the transparent texels of foliage faces (material channel in UV0, texture
// coordinates in UV1, as in the main pass).
fn foliage_discard(in: VertexOutput) {
    var material = vec2<f32>(0.0, -1.0);
@if(VERTEX_UVS_A)
    material = in.uv;
    var tex_uv = vec2<f32>(0.0, 0.0);
@if(VERTEX_UVS_B)
    tex_uv = in.uv_b;
    let layer = i32(max(material.y, 0.0) + 0.5);
    let a = textureSample(block_tex, block_smp, tex_uv, layer).a;
    let is_cut = abs(material.x - MATERIAL_FOLIAGE_CUT) < 0.5;
    if is_cut && material.y >= 0.0 && a < FOLIAGE_CUTOFF {
        discard;
    }
}

@if(PREPASS_FRAGMENT)
@fragment
fn fragment(in: VertexOutput) -> FragmentOutput {
    foliage_discard(in);
    var out: FragmentOutput;
@if(UNCLIPPED_DEPTH_ORTHO_EMULATION)
    out.frag_depth = in.unclipped_depth;
@if(NORMAL_PREPASS)
    out.normal = vec4<f32>(normalize(in.world_normal) * 0.5 + vec3<f32>(0.5), 1.0);
    return out;
}
@else
@fragment
fn fragment(in: VertexOutput) {
    foliage_discard(in);
}
