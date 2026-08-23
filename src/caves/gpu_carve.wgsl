// Cave density field on the GPU - f32 transcription of density.rs + noise.rs.
//
// Read those two files first: every function here mirrors one there, in the same
// order, and any change to the CPU implementation must land here too or --gpu will
// drift beyond its contract (>99.9% block agreement, approximate by design).
//
// The noise program arrives flattened: 20 NormalNoises become one octave list.
// Octave i owns permutation table perms[i*256 .. i*256+256]; octs[i] = (xo, yo, zo,
// input_factor); amps[i] = value_amp. ranges[n] = (first octave, count,
// value_factor bits) for noise n, indexed by the NOISE_* constants below, which
// follow the field order of CaveGen exactly.

struct Params {
    min_x: i32,
    min_z: i32,
    w: u32,
    h: u32,
    cx0: i32,
    cz0: i32,
    nx: u32,
    nz: u32,
    cy_lo: i32,
    ny: u32,
    y_lo: i32,
    nyb: u32,
    cheese_k: f32,
    top_gate: i32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var<uniform> P: Params;
@group(0) @binding(1) var<storage, read> perms: array<u32>;
@group(0) @binding(2) var<storage, read> octs: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> amps: array<f32>;
@group(0) @binding(4) var<storage, read> ranges: array<vec4<u32>>;
@group(0) @binding(5) var<storage, read_write> surf: array<i32>;
@group(0) @binding(6) var<storage, read_write> corner_vals: array<f32>;
@group(0) @binding(7) var<storage, read_write> mask: array<atomic<u32>>;

// CaveGen field order.
const NOISE_CAVE_CHEESE: u32 = 0u;
const NOISE_CAVE_LAYER: u32 = 1u;
const NOISE_CAVE_ENTRANCE: u32 = 2u;
const NOISE_SPAGHETTI_2D: u32 = 3u;
const NOISE_S2D_ELEVATION: u32 = 4u;
const NOISE_S2D_MODULATOR: u32 = 5u;
const NOISE_S2D_THICKNESS: u32 = 6u;
const NOISE_S3D_1: u32 = 7u;
const NOISE_S3D_2: u32 = 8u;
const NOISE_S3D_RARITY: u32 = 9u;
const NOISE_S3D_THICKNESS: u32 = 10u;
const NOISE_SPAG_ROUGHNESS: u32 = 11u;
const NOISE_SPAG_ROUGH_MOD: u32 = 12u;
const NOISE_NOODLE: u32 = 13u;
const NOISE_NOODLE_THICKNESS: u32 = 14u;
const NOISE_NOODLE_RIDGE_A: u32 = 15u;
const NOISE_NOODLE_RIDGE_B: u32 = 16u;
const NOISE_PILLAR: u32 = 17u;
const NOISE_PILLAR_RARENESS: u32 = 18u;
const NOISE_PILLAR_THICKNESS: u32 = 19u;

// density.rs tuning constants; keep in lockstep.
const SLOPED_CHEESE_K: f32 = 2.0;
const LAYER_SQUEEZE: f32 = 7.0;
const ENTRANCE_SHRINK: f32 = 0.32;
const TUBE_SHRINK: f32 = 0.05;
const CHEESE_SHRINK: f32 = 0.38;
const CARVE_THRESHOLD: f32 = 0.0;

// SimplexNoise.GRADIENT - grad_dot's table.
const GRAD: array<vec3<f32>, 16> = array<vec3<f32>, 16>(
    vec3<f32>(1.0, 1.0, 0.0), vec3<f32>(-1.0, 1.0, 0.0),
    vec3<f32>(1.0, -1.0, 0.0), vec3<f32>(-1.0, -1.0, 0.0),
    vec3<f32>(1.0, 0.0, 1.0), vec3<f32>(-1.0, 0.0, 1.0),
    vec3<f32>(1.0, 0.0, -1.0), vec3<f32>(-1.0, 0.0, -1.0),
    vec3<f32>(0.0, 1.0, 1.0), vec3<f32>(0.0, -1.0, 1.0),
    vec3<f32>(0.0, 1.0, -1.0), vec3<f32>(0.0, -1.0, -1.0),
    vec3<f32>(1.0, 1.0, 0.0), vec3<f32>(0.0, -1.0, 1.0),
    vec3<f32>(-1.0, 1.0, 0.0), vec3<f32>(0.0, -1.0, -1.0),
);

fn perm_at(base: u32, idx: i32) -> i32 {
    return i32(perms[base + u32(idx & 255)]);
}

fn smoothstep5(t: f32) -> f32 {
    return t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
}

// ImprovedNoise::noise with y_scale = 0 (always true for cave noises).
fn improved(oct: u32, x: f32, y: f32, z: f32) -> f32 {
    let o = octs[oct];
    let dx = x + o.x;
    let dy = y + o.y;
    let dz = z + o.z;
    let fx = floor(dx);
    let fy = floor(dy);
    let fz = floor(dz);
    let gx = i32(fx);
    let gy = i32(fy);
    let gz = i32(fz);
    let d3 = dx - fx;
    let d4 = dy - fy;
    let d5 = dz - fz;
    let base = oct * 256u;
    let i = perm_at(base, gx);
    let i1 = perm_at(base, gx + 1);
    let i2 = perm_at(base, i + gy);
    let i3 = perm_at(base, i + gy + 1);
    let i4 = perm_at(base, i1 + gy);
    let i5 = perm_at(base, i1 + gy + 1);
    let g0 = GRAD[u32(perm_at(base, i2 + gz)) & 15u];
    let g1 = GRAD[u32(perm_at(base, i4 + gz)) & 15u];
    let g2 = GRAD[u32(perm_at(base, i3 + gz)) & 15u];
    let g3 = GRAD[u32(perm_at(base, i5 + gz)) & 15u];
    let g4 = GRAD[u32(perm_at(base, i2 + gz + 1)) & 15u];
    let g5 = GRAD[u32(perm_at(base, i4 + gz + 1)) & 15u];
    let g6 = GRAD[u32(perm_at(base, i3 + gz + 1)) & 15u];
    let g7 = GRAD[u32(perm_at(base, i5 + gz + 1)) & 15u];
    let d = dot(g0, vec3<f32>(d3, d4, d5));
    let d1 = dot(g1, vec3<f32>(d3 - 1.0, d4, d5));
    let d2v = dot(g2, vec3<f32>(d3, d4 - 1.0, d5));
    let d3v = dot(g3, vec3<f32>(d3 - 1.0, d4 - 1.0, d5));
    let d4v = dot(g4, vec3<f32>(d3, d4, d5 - 1.0));
    let d5v = dot(g5, vec3<f32>(d3 - 1.0, d4, d5 - 1.0));
    let d6v = dot(g6, vec3<f32>(d3, d4 - 1.0, d5 - 1.0));
    let d7v = dot(g7, vec3<f32>(d3 - 1.0, d4 - 1.0, d5 - 1.0));
    let sx = smoothstep5(d3);
    let sy = smoothstep5(d4);
    let sz = smoothstep5(d5);
    let x00 = mix(d, d1, sx);
    let x10 = mix(d2v, d3v, sx);
    let x01 = mix(d4v, d5v, sx);
    let x11 = mix(d6v, d7v, sx);
    return mix(mix(x00, x10, sy), mix(x01, x11, sy), sz);
}

// NormalNoise::get_value via the flattened octave list. `wrap` is identity in any
// realistic coordinate range (see module doc) and is omitted.
fn normal_value(n: u32, x: f32, y: f32, z: f32) -> f32 {
    let r = ranges[n];
    let start = r.x;
    let count = r.y;
    let vf = bitcast<f32>(r.z);
    var acc = 0.0;
    for (var i = 0u; i < count; i = i + 1u) {
        let oct = start + i;
        let f = octs[oct].w;
        acc = acc + amps[oct] * improved(oct, x * f, y * f, z * f);
    }
    return acc * vf;
}

// ---- density.rs helpers, transcribed ----

fn clampd(v: f32, lo: f32, hi: f32) -> f32 {
    return clamp(v, lo, hi);
}

fn squeeze(v: f32) -> f32 {
    let c = clamp(v, -1.0, 1.0);
    return c / 2.0 - c * c * c / 24.0;
}

fn y_clamped_gradient(y: f32, from_y: f32, to_y: f32, from_v: f32, to_v: f32) -> f32 {
    if (y <= from_y) { return from_v; }
    if (y >= to_y) { return to_v; }
    return from_v + (to_v - from_v) * ((y - from_y) / (to_y - from_y));
}

fn rarity_2d(v: f32) -> f32 {
    if (v < -0.75) { return 0.5; }
    if (v < -0.5) { return 0.75; }
    if (v < 0.5) { return 1.0; }
    if (v < 0.75) { return 2.0; }
    return 3.0;
}

fn rarity_3d(v: f32) -> f32 {
    if (v < -0.5) { return 0.75; }
    if (v < 0.0) { return 1.0; }
    if (v < 0.5) { return 1.5; }
    return 2.0;
}

fn s2d_thickness_modulator(x: f32, y: f32, z: f32) -> f32 {
    return -0.95 + -0.35000000000000003 * normal_value(NOISE_S2D_THICKNESS, x * 2.0, y, z * 2.0);
}

fn spaghetti_roughness_function(x: f32, y: f32, z: f32) -> f32 {
    let a = -0.05 + -0.05 * normal_value(NOISE_SPAG_ROUGH_MOD, x, y, z);
    let b = -0.4 + abs(normal_value(NOISE_SPAG_ROUGHNESS, x, y, z));
    return a * b;
}

fn spaghetti_2d(x: f32, y: f32, z: f32) -> f32 {
    let modv = normal_value(NOISE_S2D_MODULATOR, x * 2.0, y, z * 2.0);
    let d = rarity_2d(modv);
    let wss = d * abs(normal_value(NOISE_SPAGHETTI_2D, x / d, y / d, z / d));
    let thick = s2d_thickness_modulator(x, y, z);
    let arg1 = wss + 0.083 * thick;
    let elev = 8.0 * normal_value(NOISE_S2D_ELEVATION, x, 0.0, z);
    let ycg = y_clamped_gradient(y, -64.0, 320.0, 8.0, -40.0);
    let cube_arg = abs(elev + ycg) + thick;
    let arg2 = cube_arg * cube_arg * cube_arg;
    return clampd(max(arg1, arg2), -1.0, 1.0);
}

fn entrances(x: f32, y: f32, z: f32) -> f32 {
    let depth_bias = y_clamped_gradient(y, -40.0, 30.0, 0.0, ENTRANCE_SHRINK);
    let arg_a = (0.37 + normal_value(NOISE_CAVE_ENTRANCE, x * 0.75, y * 1.1, z * 0.75))
        + y_clamped_gradient(y, -10.0, 30.0, 0.3, 0.0)
        + depth_bias;
    let rarity_in = normal_value(NOISE_S3D_RARITY, x * 2.0, y, z * 2.0);
    let d = rarity_3d(rarity_in);
    let s1 = d * abs(normal_value(NOISE_S3D_1, x / d, y / d, z / d));
    let s2 = d * abs(normal_value(NOISE_S3D_2, x / d, y / d, z / d));
    let tube_shrink = y_clamped_gradient(y, -40.0, 30.0, 0.0, TUBE_SHRINK);
    let s3d_thick = -0.0765
        + -0.011499999999999996 * normal_value(NOISE_S3D_THICKNESS, x, y, z)
        + tube_shrink;
    let clamped = clampd(max(s1, s2) + s3d_thick, -1.0, 1.0);
    let arg_b = spaghetti_roughness_function(x, y, z) + clamped;
    return min(arg_a, arg_b);
}

fn pillars(x: f32, y: f32, z: f32) -> f32 {
    let a = 2.0 * normal_value(NOISE_PILLAR, x * 25.0, y * 0.3, z * 25.0)
        + (-1.0 - normal_value(NOISE_PILLAR_RARENESS, x, y, z));
    let t = 0.55 + 0.55 * normal_value(NOISE_PILLAR_THICKNESS, x, y, z);
    return a * (t * t * t);
}

fn combined_density(x: f32, y: f32, z: f32) -> f32 {
    let entr = entrances(x, y, z);
    let layer = normal_value(NOISE_CAVE_LAYER, x, y * 8.0, z);
    let cheese_noise = clampd(
        0.27 + normal_value(NOISE_CAVE_CHEESE, x, y * 0.6666666666666666, z),
        -1.0,
        1.0,
    );
    let depth_k = y_clamped_gradient(y, -40.0, 30.0, 5.0, 0.5) * (P.cheese_k / SLOPED_CHEESE_K);
    let cheese_bias = clampd(1.5 - 0.64 * depth_k, 0.0, 0.5);
    let cheese_term = LAYER_SQUEEZE * layer * layer + cheese_noise + cheese_bias + CHEESE_SHRINK;
    let t1 = min(cheese_term, entr);
    let spag = spaghetti_2d(x, y, z) + spaghetti_roughness_function(x, y, z);
    let t2 = min(t1, spag);
    let p = pillars(x, y, z);
    var pillars_gated = p;
    if (p < 0.03) { pillars_gated = -1.0e6; }
    let cave = max(t2, pillars_gated);
    let yg1 = y_clamped_gradient(y, -64.0, -40.0, 0.0, 1.0);
    let yg2 = y_clamped_gradient(y, 240.0, 256.0, 1.0, 0.0);
    let inner = 0.1171875 + yg1 * (-0.1171875 + (-0.078125 + yg2 * (0.078125 + cave)));
    return squeeze(0.64 * inner);
}

fn noodle_density(x: f32, y: f32, z: f32) -> f32 {
    let in_range = y >= -60.0 && y < 321.0;
    var toggle = -1.0;
    if (in_range) { toggle = normal_value(NOISE_NOODLE, x, y, z); }
    if (toggle < 0.0) { return 64.0; }
    var thickness = 0.0;
    if (in_range) {
        thickness = -0.07500000000000001
            + -0.025 * normal_value(NOISE_NOODLE_THICKNESS, x, y, z);
    }
    let s = 2.6666666666666665;
    var ra = 0.0;
    var rb = 0.0;
    if (in_range) {
        ra = normal_value(NOISE_NOODLE_RIDGE_A, x * s, y * s, z * s);
        rb = normal_value(NOISE_NOODLE_RIDGE_B, x * s, y * s, z * s);
    }
    return thickness + 1.5 * max(abs(ra), abs(rb));
}

// ---- kernels ----

// One thread per lattice corner: world position (cx0+ix)*4, (cy_lo+iy)*8, (cz0+iz)*4.
@compute @workgroup_size(64)
fn corners(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    let total = P.nx * P.nz * P.ny;
    if (idx >= total) { return; }
    let iy = idx % P.ny;
    let rest = idx / P.ny;
    let iz = rest % P.nz;
    let ix = rest / P.nz;
    let wx = f32((P.cx0 + i32(ix)) * 4);
    let wy = f32((P.cy_lo + i32(iy)) * 8);
    let wz = f32((P.cz0 + i32(iz)) * 4);
    corner_vals[idx] = combined_density(wx, wy, wz);
}

fn corner_at(ix: u32, iy: u32, iz: u32) -> f32 {
    return corner_vals[(ix * P.nz + iz) * P.ny + iy];
}

// One thread per (column, y): mirror of the CPU carve loop's per-block body.
@compute @workgroup_size(256)
fn blocks(@builtin(global_invocation_id) gid: vec3<u32>) {
    let col = gid.x;
    if (col >= P.w * P.h) { return; }
    let by = P.y_lo + i32(gid.y);
    let lx = col / P.h;
    let lz = col % P.h;
    let bx = P.min_x + i32(lx);
    let bz = P.min_z + i32(lz);

    // Surface gate: carve only below surf - top_gate, exactly as the CPU loop.
    let top = surf[lx * P.h + lz] - P.top_gate;
    if (by > top) { return; }

    // The block's 4x8x4 cell and its fractional position inside it.
    let cx = i32(floor(f32(bx) / 4.0));
    let cy = i32(floor(f32(by) / 8.0));
    let cz = i32(floor(f32(bz) / 4.0));
    let ix = u32(cx - P.cx0);
    let iy = u32(cy - P.cy_lo);
    let iz = u32(cz - P.cz0);
    let fx = (f32(bx) - f32(cx * 4)) / 4.0;
    let fy = (f32(by) - f32(cy * 8)) / 8.0;
    let fz = (f32(bz) - f32(cz * 4)) / 4.0;

    let n000 = corner_at(ix, iy, iz);
    let n100 = corner_at(ix + 1u, iy, iz);
    let n010 = corner_at(ix, iy + 1u, iz);
    let n110 = corner_at(ix + 1u, iy + 1u, iz);
    let n001 = corner_at(ix, iy, iz + 1u);
    let n101 = corner_at(ix + 1u, iy, iz + 1u);
    let n011 = corner_at(ix, iy + 1u, iz + 1u);
    let n111 = corner_at(ix + 1u, iy + 1u, iz + 1u);
    let xz00 = mix(n000, n010, fy);
    let xz10 = mix(n100, n110, fy);
    let xz01 = mix(n001, n011, fy);
    let xz11 = mix(n101, n111, fy);
    let z0v = mix(xz00, xz10, fx);
    let z1v = mix(xz01, xz11, fx);
    let combined = mix(z0v, z1v, fz);

    // Carve iff min(combined, noodle) <= 0; noodle only evaluated when needed,
    // same short-circuit as the CPU.
    var carve = combined <= CARVE_THRESHOLD;
    if (!carve) {
        carve = noodle_density(f32(bx), f32(by), f32(bz)) <= 0.0;
    }
    if (carve) {
        let bit = gid.y * (P.w * P.h) + col;
        atomicOr(&mask[bit / 32u], 1u << (bit % 32u));
    }
}
