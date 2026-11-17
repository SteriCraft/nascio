// Display curvature and per-pixel sun (render only).
//
// Vertex: every vertex is lowered by k·d², with k = 1 / (2 R_vis) and d the horizontal distance
// to the camera, whose render-space position comes from the uniform (the shadow pass has the
// light as its view). Normals stay the flat-world normals: lighting happens in the flat world,
// where the sun direction depends on the position (§2.4).
//
// Fragment: the sun is computed per pixel from the fragment's world position (latitude from z,
// hour angle from x, δ_eff and the seam band from the seam distance), so the terminator, the
// seam twilight band and distant sunsets show on the horizon. Direct light fades in from 0° to
// +10° of e_vis, sky light from −6° to +10° (§2.5). Shadows come from Bevy's cascades of the
// camera's sun (the sun barely moves over the shadow range). Clouds (max of c_min(s) and the
// weather) dim the direct light. Seasonal snow covers every face but the downward ones (the
// sides of the voxel steps match their tops); the snow cover of the 160 m climate cell is
// corrected by the lapse rate between the fragment's height and the cell's mean height, so
// the snow line follows the fine relief. Haze and the far edge of the relief use the same air
// as the sky dome (`atmosphere` below), so they match it at every height (pre-M6).

import bevy_pbr::render::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput, FragmentOutput, decompress_vertex},
    mesh_view_bindings::{view, lights},
    mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT,
    view_transformations::{position_world_to_clip, position_world_to_view},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, main_pass_post_lighting_processing},
    shadows,
};

struct Curved {
    // x: k = 1 / (2 R_vis), 0 when flat; y, z, w: camera position in render space.
    drop: vec4<f32>,
    // x, z: camera world position (m, wrapped); z: hour angle at the camera (rad);
    // w: declination δ(N) (rad).
    sun: vec4<f32>,
    // s_fade, s_dark, s_light, s_clear (m).
    seam: vec4<f32>,
    // x: shadows (0/1); y: direct light scale; z: sky light scale; w: night floor.
    light: vec4<f32>,
    // x: camera world height (m); y: lapse rate (°C/m); z: snow cover per °C; w: unused.
    snow: vec4<f32>,
    // xyz: towards the visible sun at the camera; w: e_vis at the camera (rad).
    air: vec4<f32>,
    // x: cloud cover at the camera; y, z: horizontal distances where the relief starts and
    // ends fading into the sky (m); w: forced weather cloud cover, < 0 when not forced.
    haze: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100) var<storage, read> curved: Curved;
// r: weather cloud cover, g: snow cover, b: temperature ((T + 50 °C) / 100), a: mean height of
// the climate cell (/ 3 500 m), over the whole world (repeat sampler).
@group(constants::MATERIAL_BIND_GROUP) @binding(101) var climate_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102) var climate_smp: sampler;
// Colour of the world's surface (repeat sampler), for the ground beyond the relief.
@group(constants::MATERIAL_BIND_GROUP) @binding(103) var ground_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(104) var ground_smp: sampler;

const WORLD: f32 = 100000.0;
const TAU: f32 = 6.2831855;
const HALF_PI: f32 = 1.5707964;
const OBLIQUITY: f32 = 0.40910518;
const SEAM_MARGIN: f32 = 0.06981317; // 4°
const LIGHT_FADE: f32 = 0.17453292; // 10°
const SKY_LOW: f32 = -0.10471976; // −6°
const CLIMATE_HEIGHT: f32 = 3500.0;

// <atmosphere> Identical in sky.wesl and curvature.wesl (checked by a test in sky.rs).
//
// Air seen from the camera: an exponential atmosphere (scale height AIR_H) over the displayed
// ground, lowered by k·d² like every mesh. A ray above the limb gathers the air it crosses,
// from its lowest point (Chapman-like airmass); a ray below the limb ends on the ground. The
// sky dome, the haze on the relief and the far edge of the relief all use these functions,
// so the relief melts into the sky without a seam. Climbing, the column of air above the
// camera thins: the blue fades to black and only a thin band stays on the limb, up to orbit.
const AIR_H: f32 = 8000.0;
// Optical depth of the vertical column of air at sea level.
const AIR_TAU: f32 = 0.3;
// Clear sky: saturation and gain applied before the tone mapping, so the tone-mapped sky
// keeps the blue of the untone-mapped M4 dome (the haze uses the same colour: no seam).
const AIR_SATURATION: f32 = 1.3;
const AIR_SATURATION_ZENITH: f32 = 1.9;
const AIR_GAIN: f32 = 1.6;
// The textured ground seen beyond the relief fades out between 80 % and 100 % of half the
// world (never the same place twice across the torus) into this mean albedo.
const AIR_WORLD: f32 = 100000.0;
const AIR_FAR_ALBEDO: vec3<f32> = vec3<f32>(0.05, 0.09, 0.13);
const AIR_DEG: f32 = 0.017453292;
// The camera leaves the cloud deck between these heights (m).
const AIR_CLOUD_LOW: f32 = 3000.0;
const AIR_CLOUD_HIGH: f32 = 6000.0;

struct Air {
    // Towards the visible sun at the camera.
    sun: vec3<f32>,
    // e_vis at the camera (rad).
    e: f32,
    // Cloud cover at the camera.
    cloud: f32,
    // Camera height (m).
    h: f32,
    // k = 1 / (2 R_vis), 0 when flat.
    k: f32,
    // Camera world x and z (m, wrapped).
    wx: f32,
    wz: f32,
}

fn air_density(h: f32) -> f32 {
    return exp(-max(h, 0.0) / AIR_H);
}

// Mean density along a straight path between heights `a` and `b`.
fn air_mean_density(a: f32, b: f32) -> f32 {
    let lo = max(min(a, b), 0.0);
    let hi = max(max(a, b), 0.0);
    if hi - lo < 1.0 {
        return air_density(0.5 * (lo + hi));
    }
    return AIR_H * (air_density(lo) - air_density(hi)) / (hi - lo);
}

// Airmass of a horizontal ray from sea level, √(π R / 2 H) with R = 1 / (2k).
fn air_horizontal_mass(k: f32) -> f32 {
    if k <= 0.0 {
        return 40.0;
    }
    return min(sqrt(3.1415927 / (4.0 * k * AIR_H)), 40.0);
}

// Cloud cover seen from the camera: none above the cloud deck.
fn air_cloud(a: Air) -> f32 {
    return a.cloud * (1.0 - smoothstep(AIR_CLOUD_LOW, AIR_CLOUD_HIGH, a.h));
}

// Colour of opaque air towards `d`; `up` from 0 (horizontal) to 1 (zenith).
fn air_colour(a: Air, d: vec3<f32>, up: f32) -> vec3<f32> {
    let night_k = smoothstep(-18.0 * AIR_DEG, -4.0 * AIR_DEG, a.e);
    let day_k = smoothstep(-2.0 * AIR_DEG, 15.0 * AIR_DEG, a.e);
    let zen = mix(mix(vec3<f32>(0.002, 0.003, 0.009), vec3<f32>(0.05, 0.07, 0.2), night_k),
                  vec3<f32>(0.15, 0.32, 0.72), day_k);
    var hor = mix(mix(vec3<f32>(0.006, 0.008, 0.018), vec3<f32>(0.3, 0.25, 0.3), night_k),
                  vec3<f32>(0.62, 0.74, 0.9), day_k);
    // Twilight glow on the sun's side.
    let sun_side = max(dot(normalize(d.xz + vec2<f32>(1e-6)), normalize(a.sun.xz + vec2<f32>(1e-6))), 0.0);
    hor = mix(hor, vec3<f32>(0.95, 0.48, 0.22), night_k * (1.0 - day_k) * pow(sun_side, 3.0));
    let height = pow(clamp(up, 0.0, 1.0), 0.45);
    var col = mix(hor, zen, height);
    let lum = dot(col, vec3<f32>(0.2126, 0.7152, 0.0722));
    let saturation = mix(AIR_SATURATION, AIR_SATURATION_ZENITH, height);
    col = max(vec3<f32>(lum) + (col - vec3<f32>(lum)) * saturation, vec3<f32>(0.0)) * AIR_GAIN;
    // Overcast: grey, lit by the diffuse light of e_vis (§2.7: grey day, black night).
    let sky_light = smoothstep(-6.0 * AIR_DEG, 10.0 * AIR_DEG, a.e);
    let grey = vec3<f32>(0.5, 0.52, 0.56) * (0.012 + 0.988 * sky_light);
    return mix(col, grey, air_cloud(a));
}

// The ground seen beyond the relief meshes, `x` m away (horizontally) along `d`: the colour
// of the world's surface (`ground_tex`, declared by each shader), lit by the sun at the camera.
fn air_ground(a: Air, d: vec3<f32>, x: f32) -> vec3<f32> {
    let dir = d.xz / max(length(d.xz), 1e-5);
    let p = vec2<f32>(a.wx, a.wz) + dir * x;
    let albedo = textureSampleLevel(ground_tex, ground_smp, p / AIR_WORLD, 0.0).rgb;
    let base = mix(albedo, AIR_FAR_ALBEDO, smoothstep(0.4 * AIR_WORLD, 0.5 * AIR_WORLD, x));
    let sky = smoothstep(-6.0 * AIR_DEG, 10.0 * AIR_DEG, a.e);
    let direct = smoothstep(0.0, 10.0 * AIR_DEG, a.e) * max(a.sun.y, 0.0);
    return base * (0.02 + 0.2 * sky + 0.9 * direct * (1.0 - 0.8 * air_cloud(a)));
}

// Horizontal distance at which the ray of slope `t` (< 0) meets the displayed ground, or −1.
fn air_ground_hit(a: Air, t: f32) -> f32 {
    let h = max(a.h, 0.0);
    if t >= 0.0 {
        return -1.0;
    }
    if a.k <= 0.0 {
        return h / max(-t, 1e-6);
    }
    let disc = t * t - 4.0 * a.k * h;
    if disc < 0.0 {
        return -1.0;
    }
    return (-t - sqrt(disc)) / (2.0 * a.k);
}

// Whether `d` points below the limb (at the ground).
fn air_below_limb(a: Air, d: vec3<f32>) -> bool {
    return air_ground_hit(a, d.y / max(length(d.xz), 1e-5)) >= 0.0;
}

// Haze added to `colour` seen at `len` m (displayed) along `d`, at world height `hf`.
fn air_haze(a: Air, d: vec3<f32>, len: f32, hf: f32, colour: vec3<f32>) -> vec3<f32> {
    let tau = AIR_TAU * air_mean_density(a.h, hf) * len / AIR_H;
    let full = 1.0 - exp(-AIR_TAU * air_horizontal_mass(a.k));
    let haze = min((1.0 - exp(-tau)) / full, 1.2);
    return colour * exp(-tau) + air_colour(a, d, max(d.y, 0.0)) * haze;
}

// Colour of the sky (or of the far ground) towards `d`, without the sun, Moon and aurorae.
fn atmosphere(a: Air, d: vec3<f32>) -> vec3<f32> {
    let horiz = max(length(d.xz), 1e-5);
    let t = d.y / horiz;
    let h = max(a.h, 0.0);
    let m0 = air_horizontal_mass(a.k);
    let x_hit = air_ground_hit(a, t);
    if x_hit >= 0.0 {
        return air_haze(a, d, x_hit * sqrt(1.0 + t * t), 0.0, air_ground(a, d, x_hit));
    }
    var tau: f32;
    var tau_ref: f32;
    if t >= 0.0 {
        let m = 1.0 / (d.y + 1.0 / m0);
        tau = AIR_TAU * air_density(h) * m;
        tau_ref = AIR_TAU * m;
    } else {
        // Down to the perigee and out again; the part above the camera is not crossed twice.
        let hp = h - t * t / (4.0 * max(a.k, 1e-12));
        tau = AIR_TAU * m0 * (2.0 * air_density(hp) - air_density(h));
        tau_ref = AIR_TAU * m0;
    }
    // Relative to the same direction seen from sea level (the sky of the ground is unchanged).
    let glow = min((1.0 - exp(-tau)) / (1.0 - exp(-tau_ref)), 1.2);
    return air_colour(a, d, max(d.y, 0.0)) * glow;
}
// </atmosphere>

// Snow cover at a fragment of world height `h`: the cell's cover, less snow below the cell's
// mean height, more above it where it is cold enough (no summer snow on warm peaks).
fn snow_cover(climate: vec4<f32>, h: f32) -> f32 {
    let t_cell = climate.b * 100.0 - 50.0;
    let dt = -curved.snow.y * (h - climate.a * CLIMATE_HEIGHT);
    let t_local = t_cell + dt;
    let k = curved.snow.z;
    if dt > 0.0 {
        return clamp(climate.g - k * dt, 0.0, 1.0);
    }
    let cold = 1.0 - smoothstep(0.0, 5.0, t_local);
    return clamp(climate.g - k * dt * cold, 0.0, 1.0);
}

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

@if(VERTEX_NORMALS)
    out.world_normal = mesh_functions::mesh_normal_local_to_world(v.normal, vertex_in.instance_index);

@if(VERTEX_UVS_A)
    out.uv = v.uv;
@if(VERTEX_UVS_B)
    out.uv_b = v.uv_b;
@if(VERTEX_COLORS)
    out.color = v.color;
@if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = vertex_in.instance_index;
@if(VISIBILITY_RANGE_DITHER)
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex_in.instance_index, world_from_local[3]);

    return out;
}

struct LocalSun {
    // Towards the visible sun, world axes (x east, y up, z south).
    dir: vec3<f32>,
    e_vis: f32,
    // Seam distance, m.
    s: f32,
}

// Sun at a point `dx` metres east of the camera, at wrapped world z (spec §2.4, §2.5).
fn local_sun(dx: f32, z_world: f32) -> LocalSun {
    let z = z_world - floor(z_world / WORLD) * WORLD;
    let phi = asin(clamp((50000.0 - z) / 50000.0, -1.0, 1.0));
    let s = min(z, WORLD - z);
    let w = smoothstep(0.0, curved.seam.x, s);
    let dec = curved.sun.w * w;
    let h = curved.sun.z + TAU * dx / WORLD;
    let e = asin(clamp(sin(phi) * sin(dec) + cos(phi) * cos(dec) * cos(h), -1.0, 1.0));
    let a = atan2(-cos(dec) * sin(h), sin(dec) * cos(phi) - cos(dec) * cos(h) * sin(phi));
    let r = 1.0 - smoothstep(curved.seam.y, curved.seam.z, s);
    let e_vis = e - r * (HALF_PI - abs(phi) + OBLIQUITY * w + SEAM_MARGIN);
    let c = cos(e_vis);
    return LocalSun(vec3<f32>(c * sin(a), sin(e_vis), -c * cos(a)), e_vis, s);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    // Water is tagged by a vertex alpha below 1 (chunk water is translucent, horizon water is
    // opaque but tagged); read it before alpha_discard forces opaque alphas to 1.
    var tag = 1.0;
@if(VERTEX_COLORS)
    tag = in.color.a;
    let is_water = tag < 0.99;
    pbr_input.material.base_color = alpha_discard(
        pbr_input.material.flags,
        pbr_input.material.alpha_cutoff,
        pbr_input.material.base_color
    );
    var albedo = pbr_input.material.base_color.rgb;
    var alpha = pbr_input.material.base_color.a;
    let n = pbr_input.N;
    // Chunk water: its alpha grows with the depth of the column (mesher, M6), from light
    // rivers to a nearly opaque sea whose floor may not be loaded; darken it the same way
    // towards the deep-water tone of the horizon.
    if is_water && alpha < 0.99 {
        let deep = clamp((alpha - 0.4) / 0.57, 0.0, 1.0);
        albedo *= mix(0.95, 0.5, deep);
    }

    // World position of the fragment (camera-relative offsets are exact across the seam).
    let rel = in.world_position.xz - curved.drop.yw;
    let wx = curved.sun.x + rel.x;
    let wz = curved.sun.y + rel.y;
    let sun = local_sun(rel.x, wz);

    let climate = textureSample(climate_tex, climate_smp, vec2<f32>(wx, wz) / WORLD);
    let c_min = 1.0 - smoothstep(curved.seam.z, curved.seam.w, sun.s);
    // `haze.w` >= 0 forces the weather cloud cover (captures).
    let weather = select(climate.r, curved.haze.w, curved.haze.w >= 0.0);
    let cloud = max(c_min, weather);

    // Seasonal snow on land faces (not on water), sides included, never on downward faces.
    if !is_water {
        let h = in.world_position.y + curved.drop.x * dot(rel, rel) - curved.drop.z + curved.snow.x;
        let facing = smoothstep(-0.5, -0.2, n.y);
        let snow = smoothstep(0.15, 0.6, snow_cover(climate, h)) * facing;
        albedo = mix(albedo, vec3<f32>(0.86, 0.89, 0.93), snow);
    }

    let direct = smoothstep(0.0, LIGHT_FADE, sun.e_vis) * curved.light.y * (1.0 - 0.8 * cloud);
    let sky = smoothstep(SKY_LOW, LIGHT_FADE, sun.e_vis);
    let warm = smoothstep(0.0, 0.35, sun.e_vis);
    let sun_color = mix(vec3<f32>(1.0, 0.6, 0.35), vec3<f32>(1.0, 0.96, 0.9), warm);
    let sky_color = mix(vec3<f32>(0.42, 0.47, 0.56), vec3<f32>(0.5, 0.52, 0.55), cloud);
    let night = vec3<f32>(0.035, 0.045, 0.08) * curved.light.w;
    var ambient = night + sky_color * sky * curved.light.z;

    var shadow = 1.0;
    if curved.light.x > 0.5 && lights.n_directional_lights > 0u {
        let light = lights.directional_lights[0];
        if (light.flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u {
            let view_z = position_world_to_view(in.world_position.xyz).z;
            shadow = shadows::fetch_directional_shadow(0u, in.world_position, pbr_input.world_normal, view_z, in.position.xy);
        }
    }
    let lambert = max(dot(n, sun.dir), 0.0);
    var color = albedo * (ambient + sun_color * direct * lambert * shadow);

    // Water: sun glint and a little sky reflection.
    if is_water {
        let r = reflect(-sun.dir, n);
        let glint = pow(max(dot(r, pbr_input.V), 0.0), 80.0) * direct * shadow;
        let fresnel = pow(1.0 - max(dot(n, pbr_input.V), 0.0), 4.0);
        color += sun_color * glint * 2.0 + sky_color * sky * fresnel * 0.3;
    }

    // Haze, then the far edge of the relief fades into the sky dome: the same air (no seam
    // between the relief, the curvature of the planet and the sky).
    let to_frag = in.world_position.xyz - curved.drop.yzw;
    let len = length(to_frag);
    let d = to_frag / max(len, 1e-3);
    let air = Air(curved.air.xyz, curved.air.w, curved.haze.x, curved.snow.x, curved.drop.x, curved.sun.x, curved.sun.y);
    let hf = in.world_position.y + curved.drop.x * dot(rel, rel) - curved.drop.z + curved.snow.x;
    color = air_haze(air, d, len, hf, color);
    let edge = smoothstep(curved.haze.y, curved.haze.z, length(rel));
    if edge > 0.0 {
        color = mix(color, atmosphere(air, d), edge);
    }

    var out: FragmentOutput;
    out.color = main_pass_post_lighting_processing(pbr_input, vec4<f32>(color, alpha));
    return out;
}
