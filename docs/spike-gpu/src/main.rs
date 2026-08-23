//! Spike: the three numbers the Phase 2 GPU plan is missing, measured on THIS machine.
//!
//! 1. wgpu adapter + device init cost (the per-context tax the plan argues about)
//! 2. small-dispatch round-trip latency (the per-region launch tax)
//! 3. f32 improved-Perlin throughput on the GPU, in the same "octave evals" unit the
//!    CPU was measured in (combined_density = 54 octave evals in 1194 ns on one core,
//!    so 45.2 M evals/s/core, ~1.09 G/s across 24 ideal cores)
//!
//! Deliberately NOT a port of arnis noise: same arithmetic class (integer hash, 8
//! gradient dots, quintic fade, trilerp per octave), so it measures the hardware, not
//! the port. Kernel math is f32 - the plan already established f64 is infeasible.

use std::time::Instant;

// Hybrid-graphics opt-in: drivers check the exe's EXPORT table for these.
#[no_mangle]
pub static NvOptimusEnablement: u32 = 1;
#[no_mangle]
pub static AmdPowerXpressRequestHighPerformance: u32 = 1;


const OCTAVES: u32 = 54;
const INVOCATIONS: u64 = 1 << 24;

const SHADER: &str = r#"
struct Params { seed: u32, octaves: u32, _p0: u32, _p1: u32 }
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read_write> out: array<f32>;

fn hash3(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    var h: u32 = seed;
    h = h ^ (u32(x) * 0x9E3779B9u);
    h = h ^ (u32(y) * 0x85EBCA6Bu);
    h = h ^ (u32(z) * 0xC2B2AE35u);
    h = h ^ (h >> 16u);
    h = h * 0x7FEB352Du;
    h = h ^ (h >> 15u);
    return h;
}

fn grad(h: u32, x: f32, y: f32, z: f32) -> f32 {
    let g = h & 15u;
    let u = select(y, x, g < 8u);
    let v = select(select(z, y, g < 4u), x, g == 12u || g == 14u);
    let su = select(u, -u, (g & 1u) != 0u);
    let sv = select(v, -v, (g & 2u) != 0u);
    return su + sv;
}

fn fade(t: f32) -> f32 { return t * t * t * (t * (t * 6.0 - 15.0) + 10.0); }

fn perlin(px: f32, py: f32, pz: f32, seed: u32) -> f32 {
    let xf = floor(px); let yf = floor(py); let zf = floor(pz);
    let xi = i32(xf); let yi = i32(yf); let zi = i32(zf);
    let x = px - xf; let y = py - yf; let z = pz - zf;
    let u = fade(x); let v = fade(y); let w = fade(z);
    let n000 = grad(hash3(xi, yi, zi, seed), x, y, z);
    let n100 = grad(hash3(xi + 1, yi, zi, seed), x - 1.0, y, z);
    let n010 = grad(hash3(xi, yi + 1, zi, seed), x, y - 1.0, z);
    let n110 = grad(hash3(xi + 1, yi + 1, zi, seed), x - 1.0, y - 1.0, z);
    let n001 = grad(hash3(xi, yi, zi + 1, seed), x, y, z - 1.0);
    let n101 = grad(hash3(xi + 1, yi, zi + 1, seed), x - 1.0, y, z - 1.0);
    let n011 = grad(hash3(xi, yi + 1, zi + 1, seed), x, y - 1.0, z - 1.0);
    let n111 = grad(hash3(xi + 1, yi + 1, zi + 1, seed), x - 1.0, y - 1.0, z - 1.0);
    let x00 = mix(n000, n100, u); let x10 = mix(n010, n110, u);
    let x01 = mix(n001, n101, u); let x11 = mix(n011, n111, u);
    return mix(mix(x00, x10, v), mix(x01, x11, v), w);
}

@compute @workgroup_size(256)
fn noise_bench(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x + gid.y * 65536u;
    let p = vec3<f32>(f32(i % 4096u), f32((i / 4096u) % 4096u), f32(i / 16777216u));
    var acc = 0.0;
    var freq = 0.011;
    for (var o = 0u; o < params.octaves; o = o + 1u) {
        acc = acc + perlin(p.x * freq, p.y * freq, p.z * freq, params.seed + o);
        freq = freq * 1.37;
    }
    out[i & 4095u] = acc;
}

@compute @workgroup_size(64)
fn tiny(@builtin(global_invocation_id) gid: vec3<u32>) {
    out[gid.x & 4095u] = f32(gid.x) + f32(params.seed);
}
"#;

fn main() {
    // FINDING baked in: PowerPreference::HighPerformance picked the Intel iGPU over
    // the RTX 5080 on this laptop, so the real integration must enumerate adapters
    // and choose by name/type, never trust the preference flag. ARNIS_SPIKE_ADAPTER
    // selects a substring here; default prefers a discrete adapter.
    let t0 = Instant::now();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
    let all: Vec<wgpu::Adapter> = instance
        .enumerate_adapters(wgpu::Backends::all())
        .into_iter()
        .collect();
    for a in &all {
        let i = a.get_info();
        println!("  candidate: {} ({:?} / {:?})", i.name, i.device_type, i.backend);
    }
    let want = std::env::var("ARNIS_SPIKE_ADAPTER").unwrap_or_default().to_lowercase();
    let adapter = all
        .into_iter()
        .filter(|a| {
            want.is_empty() || a.get_info().name.to_lowercase().contains(&want)
        })
        .max_by_key(|a| {
            let i = a.get_info();
            let class = match i.device_type {
                wgpu::DeviceType::DiscreteGpu => 3,
                wgpu::DeviceType::IntegratedGpu => 2,
                _ => 0,
            };
            // Prefer Vulkan over DX12 over the rest when the same card shows twice.
            let backend = match i.backend {
                wgpu::Backend::Vulkan => 2,
                wgpu::Backend::Dx12 => 1,
                _ => 0,
            };
            class * 10 + backend
        })
        .expect("no adapter matched");
    let t_adapter = t0.elapsed();
    let info = adapter.get_info();
    let t1 = Instant::now();
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None))
            .expect("no device");
    let t_device = t1.elapsed();
    println!(
        "adapter: {} ({:?} / {:?})",
        info.name, info.device_type, info.backend
    );
    println!(
        "INIT adapter_ms={:.1} device_ms={:.1} total_ms={:.1}",
        t_adapter.as_secs_f64() * 1e3,
        t_device.as_secs_f64() * 1e3,
        (t_adapter + t_device).as_secs_f64() * 1e3
    );

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let out_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 4096 * 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let params_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let read_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 4096 * 4,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: out_buf.as_entire_binding(),
            },
        ],
    });
    let pipe_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });
    let make_pipe = |entry: &str| {
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: Some(&pipe_layout),
            module: &shader,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            cache: None,
        })
    };
    let noise_pipe = make_pipe("noise_bench");
    let tiny_pipe = make_pipe("tiny");

    let run = |pipe: &wgpu::ComputePipeline, groups: u32, seed: u32, octaves: u32| -> f32 {
        queue.write_buffer(
            &params_buf,
            0,
            bytemuck::cast_slice(&[seed, octaves, 0u32, 0u32]),
        );
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(groups, 1, 1);
        }
        enc.copy_buffer_to_buffer(&out_buf, 0, &read_buf, 0, 4096 * 4);
        queue.submit([enc.finish()]);
        let slice = read_buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::Maintain::Wait);
        rx.recv().unwrap().unwrap();
        let checksum: f32 = bytemuck::cast_slice::<u8, f32>(&slice.get_mapped_range())
            .iter()
            .take(8)
            .sum();
        read_buf.unmap();
        checksum
    };

    let run2d = |pipe: &wgpu::ComputePipeline, gx: u32, gy: u32, seed: u32, octaves: u32| -> f32 {
        queue.write_buffer(
            &params_buf,
            0,
            bytemuck::cast_slice(&[seed, octaves, 0u32, 0u32]),
        );
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(gx, gy, 1);
        }
        enc.copy_buffer_to_buffer(&out_buf, 0, &read_buf, 0, 4096 * 4);
        queue.submit([enc.finish()]);
        let slice = read_buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(wgpu::Maintain::Wait);
        rx.recv().unwrap().unwrap();
        let checksum: f32 = bytemuck::cast_slice::<u8, f32>(&slice.get_mapped_range())
            .iter()
            .take(8)
            .sum();
        read_buf.unmap();
        checksum
    };

    run(&tiny_pipe, 1, 1, 0);
    let mut lat = Vec::new();
    for i in 0..20u32 {
        let t = Instant::now();
        run(&tiny_pipe, 1, i, 0);
        lat.push(t.elapsed().as_secs_f64() * 1e3);
    }
    lat.sort_by(f64::total_cmp);
    println!("ROUNDTRIP tiny median_ms={:.2} p90_ms={:.2}", lat[10], lat[18]);

    // 256 groups x 256 threads = 65,536 invocations per row; 256 rows = 16.7 M.
    // A single dimension would need 65,536 groups, one over the API limit.
    let mut check = run2d(&noise_pipe, 256, 256, 7, OCTAVES);
    let mut times = Vec::new();
    for i in 0..3u32 {
        let t = Instant::now();
        check += run2d(&noise_pipe, 256, 256, 100 + i, OCTAVES);
        times.push(t.elapsed().as_secs_f64());
    }
    times.sort_by(f64::total_cmp);
    let best = times[0];
    let evals = INVOCATIONS as f64 * OCTAVES as f64;
    println!(
        "NOISE invocations={} octaves={} best_s={:.4} evals_per_s={:.3e} (checksum {:.3})",
        INVOCATIONS,
        OCTAVES,
        best,
        evals / best,
        check
    );
    println!("CPU reference: 45.2e6 evals/s/core measured; 24-core ideal 1.09e9");
    println!(
        "GPU_vs_one_core={:.1}x GPU_vs_24_cores={:.2}x",
        (evals / best) / 45.2e6,
        (evals / best) / 1.09e9
    );
}
