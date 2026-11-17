// Sky dome (spec §2.5 – §2.8), drawn on a sphere around the camera.
//
// - Colour from the visible sun elevation at the camera: day, twilight glow towards the sun,
//   night; overcast grey under the cloud cover c = max(c_min(s), weather).
// - Sun and Moon discs (the Moon lit from the geometric sun: its phase), weighted by 1 − c.
//   The Moon is textured (NASA SVS CGI Moon Kit): near side towards the observer, lunar north
//   towards the celestial pole.
// - Aurorae: curtains on the geographic ring around the seam band (§2.7), at a few kilometres
//   of altitude. The view ray meets the displayed (lowered) shell `y = H − k·d²`, so the ring
//   is seen far away, low on the horizon. Night only (e_vis at the curtain), masked by clouds.
//   The stars are a separate mesh.

import bevy_pbr::render::{
    forward_io::VertexOutput,
    mesh_view_bindings::view,
};
// Same tone mapping as the lit materials (the relief fades into this dome).
@if(TONEMAP_IN_SHADER) import bevy_core_pipeline::tonemapping::tone_mapping;

struct Sky {
    // xyz: towards the visible sun at the camera; w: e_vis at the camera (rad).
    sun: vec4<f32>,
    // xyz: towards the Moon; w: unused.
    moon: vec4<f32>,
    // xyz: towards the geometric sun (Moon phase); w: cloud cover at the camera.
    sun_geo: vec4<f32>,
    // x, y: camera world x, z (m); z: camera height (m); w: k = 1 / (2 R_vis).
    cam: vec4<f32>,
    // x: hour angle at the camera; y: declination δ(N); z: animation time (s); w: aurora gain.
    time: vec4<f32>,
    // s_fade, s_dark, s_light, s_clear (m).
    seam: vec4<f32>,
    // x: ring peak s_aurora; y: ring outer edge; z: curtain base altitude (m); w: unused.
    aurora: vec4<f32>,
    // xyz: towards the north celestial pole; w: unused.
    pole: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> sky: Sky;
// Moon albedo map, equirectangular, longitude 0 in the middle, east to the right.
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var moon_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var moon_smp: sampler;

const WORLD: f32 = 100000.0;
const TAU: f32 = 6.2831855;
const PI: f32 = 3.1415927;
const HALF_PI: f32 = 1.5707964;
const OBLIQUITY: f32 = 0.40910518;
const DEG: f32 = 0.017453292;

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
    let col = mix(hor, zen, pow(clamp(up, 0.0, 1.0), 0.45));
    // Overcast: grey, lit by the diffuse light of e_vis (§2.7: grey day, black night).
    let sky_light = smoothstep(-6.0 * AIR_DEG, 10.0 * AIR_DEG, a.e);
    let grey = vec3<f32>(0.5, 0.52, 0.56) * (0.012 + 0.988 * sky_light);
    return mix(col, grey, air_cloud(a));
}

// The ground far below, beyond the relief meshes: dark, mostly ocean.
fn air_ground(a: Air) -> vec3<f32> {
    let light = smoothstep(-4.0 * AIR_DEG, 12.0 * AIR_DEG, a.e);
    return vec3<f32>(0.035, 0.06, 0.09) * (0.02 + 0.98 * light);
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
        return air_haze(a, d, x_hit * sqrt(1.0 + t * t), 0.0, air_ground(a));
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

// Visible sun elevation at a point `dx` m east of the camera, at world z (§2.4, §2.5).
fn e_vis_at(dx: f32, z_world: f32) -> f32 {
    let z = z_world - floor(z_world / WORLD) * WORLD;
    let phi = asin(clamp((50000.0 - z) / 50000.0, -1.0, 1.0));
    let s = min(z, WORLD - z);
    let w = smoothstep(0.0, sky.seam.x, s);
    let dec = sky.time.y * w;
    let h = sky.time.x + TAU * dx / WORLD;
    let e = asin(clamp(sin(phi) * sin(dec) + cos(phi) * cos(dec) * cos(h), -1.0, 1.0));
    let r = 1.0 - smoothstep(sky.seam.y, sky.seam.z, s);
    return e - r * (HALF_PI - abs(phi) + OBLIQUITY * w + 4.0 * DEG);
}

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.547);
}

// Value noise, periodic in x with period `px` lattice cells (the torus wraps every 100 km).
fn noise(p: vec2<f32>, px: f32) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let x0 = i.x - floor(i.x / px) * px;
    let x1 = select(x0 + 1.0, 0.0, x0 + 1.0 >= px);
    let a = hash(vec2<f32>(x0, i.y));
    let b = hash(vec2<f32>(x1, i.y));
    let c = hash(vec2<f32>(x0, i.y + 1.0));
    let e = hash(vec2<f32>(x1, i.y + 1.0));
    return mix(mix(a, b, u.x), mix(c, e, u.x), u.y);
}

// Aurora emission along the view direction `d`.
fn aurora(d: vec3<f32>) -> vec3<f32> {
    let k = sky.cam.w;
    let horiz = length(d.xz);
    var sum = vec3<f32>(0.0);
    let layers = 10;
    // Per-pixel jitter of the shells: turns the banding between them into fine noise.
    let jitter = hash(d.xy * 4096.0 + d.zx * 977.0);
    for (var layer = 0; layer < layers; layer++) {
        let h01 = (f32(layer) + jitter) / f32(layers);
        let alt = sky.aurora.z + h01 * 4000.0;
        let c0 = sky.cam.z - alt;
        if c0 >= 0.0 {
            continue;
        }
        let a = k * horiz * horiz;
        var t: f32;
        if a > 1e-12 {
            t = (-d.y + sqrt(d.y * d.y - 4.0 * a * c0)) / (2.0 * a);
        } else if d.y > 1e-4 {
            t = -c0 / d.y;
        } else {
            continue;
        }
        let dist = t * horiz;
        if dist > 45000.0 {
            continue;
        }
        let rel = d.xz * t;
        let wx = sky.cam.x + rel.x;
        let wz = sky.cam.y + rel.y;
        let zz = wz - floor(wz / WORLD) * WORLD;
        let s = min(zz, WORLD - zz);
        let ring = smoothstep(sky.seam.w, sky.aurora.x, s) * (1.0 - smoothstep(sky.aurora.x, sky.aurora.y, s));
        if ring <= 0.0 {
            continue;
        }
        let night = 1.0 - smoothstep(-12.0 * DEG, -6.0 * DEG, e_vis_at(rel.x, wz));
        // Curtains: vertical sheets, so the pattern depends on the horizontal position only
        // (folds along the ring, drifting slowly); bright lower edge, purple tops.
        let tt = sky.time.z * 0.03;
        let fold = noise(vec2<f32>(wx / 1250.0 + tt, s / 700.0), 80.0)
            * noise(vec2<f32>(wx / 200.0 - tt * 2.0, s / 260.0), 500.0);
        let sheet = smoothstep(0.18, 0.5, fold);
        let fade = exp(-dist / 30000.0);
        let profile = (1.0 - h01) * (1.0 - h01) + 0.15;
        let col = mix(vec3<f32>(0.2, 1.0, 0.5), vec3<f32>(0.6, 0.25, 0.85), smoothstep(0.35, 1.0, h01));
        sum += col * ring * night * sheet * fade * profile;
    }
    return sum * sky.time.w * 0.35;
}

fn finish(col: vec3<f32>) -> vec4<f32> {
    var out = vec4<f32>(col, 1.0);
@if(TONEMAP_IN_SHADER) {
    out = tone_mapping(out, view.color_grading);
}
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let d = normalize(in.world_position.xyz - view.world_position.xyz);
    let e = sky.sun.w;
    let air = Air(sky.sun.xyz, e, sky.sun_geo.w, sky.cam.z, sky.cam.w);
    let day_k = smoothstep(-2.0 * DEG, 15.0 * DEG, e);
    var col = atmosphere(air, d);
    // The planet hides the sun, the Moon and the aurorae below the limb.
    if air_below_limb(air, d) {
        return finish(col);
    }
    let clear = 1.0 - air_cloud(air);

    // Sun disc and halo.
    let cs = dot(d, sky.sun.xyz);
    let disc = smoothstep(cos(0.7 * DEG), cos(0.5 * DEG), cs);
    // The wide halo is scattered by the air: it fades with height.
    let halo = pow(max(cs, 0.0), 300.0) * 0.6
        + pow(max(cs, 0.0), 12.0) * 0.08 * day_k * air_density(air.h);
    let sun_col = mix(vec3<f32>(1.0, 0.55, 0.3), vec3<f32>(1.0, 0.97, 0.9), smoothstep(0.0, 12.0 * DEG, e));
    col += sun_col * (disc * 12.0 + halo) * clear * step(-1.0 * DEG, e);

    // Moon disc with its phase.
    let m = sky.moon.xyz;
    let cm = dot(d, m);
    let r_moon = 1.1 * DEG;
    if cm > cos(r_moon) {
        let off = (d - m * cm) / sin(r_moon);
        let nz = sqrt(max(1.0 - dot(off, off), 0.0));
        // The visible hemisphere faces the observer (−m).
        let normal = normalize(off - m * nz);
        let lit = max(dot(normal, sky.sun_geo.xyz), 0.0);
        let edge = smoothstep(cos(r_moon), cos(r_moon * 0.92), cm);
        // Selenographic coordinates: the sub-observer point (−m) is (0, 0), lunar north is
        // the celestial pole projected on the disc, lunar east points to the sky's west.
        var up = sky.pole.xyz - m * dot(sky.pole.xyz, m);
        if dot(up, up) < 1e-6 {
            up = vec3<f32>(0.0, 1.0, 0.0) - m * m.y;
        }
        up = normalize(up);
        let east = cross(m, up);
        let lat = asin(clamp(dot(normal, up), -1.0, 1.0));
        let lon = atan2(dot(normal, east), dot(normal, -m));
        let uv = vec2<f32>(0.5 + lon / TAU, 0.5 - lat / PI);
        let albedo = textureSampleLevel(moon_tex, moon_smp, uv, 0.0).rgb;
        let moon = albedo * 1.4 * (0.03 + 1.2 * lit);
        col = mix(col, col + moon, edge * clear);
    }

    // Aurorae are lost in a bright sky: only twilight and night skies show them.
    col += aurora(d) * clear * (1.0 - 0.95 * day_k * air_density(air.h));
    return finish(col);
}
