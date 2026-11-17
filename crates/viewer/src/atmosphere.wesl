// Air seen from the camera: an exponential atmosphere (scale height AIR_H) over the displayed
// ground, lowered by k·d² like every mesh. A ray above the limb gathers the air it crosses,
// from its lowest point (Chapman-like airmass); a ray below the limb ends on the ground. The
// sky dome, the haze on the relief and the far edge of the relief all use these functions,
// so the relief melts into the sky without a seam. Climbing, the column of air above the
// camera thins: the blue fades to black and only a thin band stays on the limb, up to orbit.
//
// One module imported by sky.wesl and curvature.wesl, so they cannot drift apart (audit R2);
// the ground texture is bound by each shader and passed in.
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
// of the world's surface (`ground_tex`, bound by each shader), lit by the sun at the camera.
fn air_ground(a: Air, d: vec3<f32>, x: f32, ground_tex: texture_2d<f32>, ground_smp: sampler) -> vec3<f32> {
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
fn atmosphere(a: Air, d: vec3<f32>, ground_tex: texture_2d<f32>, ground_smp: sampler) -> vec3<f32> {
    let horiz = max(length(d.xz), 1e-5);
    let t = d.y / horiz;
    let h = max(a.h, 0.0);
    let m0 = air_horizontal_mass(a.k);
    let x_hit = air_ground_hit(a, t);
    if x_hit >= 0.0 {
        return air_haze(a, d, x_hit * sqrt(1.0 + t * t), 0.0, air_ground(a, d, x_hit, ground_tex, ground_smp));
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
