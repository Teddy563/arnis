//! GPU evaluation of the cave density field (`--gpu`).
//!
//! The carve decision is a pure function of block coordinates: sample
//! `combined_density` at the corners of each 4x8x4 cell, trilerp per block, min with
//! per-block `noodle_density`, carve at <= 0 below the surface gate. Nothing in it
//! touches the world, which is what makes it portable to a GPU wholesale.
//!
//! Two dispatches per region:
//! 1. `corners` - `combined_density` at every lattice corner
//! 2. `blocks` - per block: trilerp the 8 corners, evaluate noodle, apply the
//!    surface gate, set one bit in a carve mask
//!
//! The bitmask comes back to the CPU, which applies it through the ordinary editor
//! path - so despeckle, decoration, sealing and every later pass are unchanged.
//!
//! Numbers travel as f32 on the GPU while the CPU reference is f64. Near the carve
//! threshold that flips occasional cells, so GPU output is APPROXIMATE by contract:
//! validated to agree with the CPU on >99.9% of blocks, never bit-pinned. The golden
//! hash gate keeps running the CPU path. Precision note: at |coord| ~5e4 an f32 ulp
//! is ~4e-3 of a block, and vanilla's `wrap` at 3.35e7 is an identity in that range,
//! so it is omitted on the GPU.
//!
//! Adapter selection is explicit enumeration - measured on the target laptop,
//! `PowerPreference::HighPerformance` chose the iGPU over the RTX 5080, and a
//! hybrid-graphics machine may hide the dGPU entirely. `--gpu dgpu|igpu|<substring>`
//! forces a class or a name; `--gpu auto` prefers discrete. Every failure path
//! returns `Err` and the caller falls back to the CPU implementation.

use super::density::CaveGen;
use super::noise::OctaveExport;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// Wall time spent inside GPU dispatches this process, in milliseconds.
///
/// Meld reads the report line and budgets workers against a GPU target the same
/// way it budgets CPU and RAM - it cannot observe an adapter from outside the
/// process, so the process says what it used.
pub(crate) static GPU_BUSY_MS: AtomicU64 = AtomicU64::new(0);

/// Fixed noise order shared with the shader; index = position in this list.
/// The WGSL references noises by these indices, so the two must move together.
const NOISE_COUNT: usize = 20;

const SHADER_SRC: &str = include_str!("gpu_carve.wgsl");

pub(crate) struct GpuCarver {
    device: wgpu::Device,
    queue: wgpu::Queue,
    corners_pipe: wgpu::ComputePipeline,
    blocks_pipe: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    // Static noise program, uploaded once.
    perms_buf: wgpu::Buffer,
    octs_buf: wgpu::Buffer,
    amps_buf: wgpu::Buffer,
    ranges_buf: wgpu::Buffer,
    pub adapter_name: String,
    seed: i64,
}

/// Uniform block shared by both kernels. Must match `Params` in the WGSL.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
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

fn flatten_noises(gen: &CaveGen) -> (Vec<u32>, Vec<[f32; 4]>, Vec<f32>, Vec<u32>) {
    // (perms, octave vec4s [xo,yo,zo,input_factor], value_amps, ranges+value_factor bits)
    let (noises, _cheese_k) = gen.gpu_export();
    assert_eq!(noises.len(), NOISE_COUNT);
    let mut perms: Vec<u32> = Vec::new();
    let mut octs: Vec<[f32; 4]> = Vec::new();
    let mut amps: Vec<f32> = Vec::new();
    // ranges: per noise 4 u32s: start, count, value_factor (f32 bits), pad
    let mut ranges: Vec<u32> = Vec::new();
    for n in noises {
        let exported: Vec<OctaveExport> = n.export_octaves();
        let start = octs.len() as u32;
        for o in &exported {
            for &b in o.perm.iter() {
                perms.push(b as u32);
            }
            octs.push([o.xo as f32, o.yo as f32, o.zo as f32, o.input_factor as f32]);
            amps.push(o.value_amp as f32);
        }
        ranges.push(start);
        ranges.push(exported.len() as u32);
        ranges.push((n.export_value_factor() as f32).to_bits());
        ranges.push(0);
    }
    (perms, octs, amps, ranges)
}

impl GpuCarver {
    fn create(selector: &str, gen: &CaveGen, seed: i64) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let sel = selector.to_lowercase();
        let adapters = instance.enumerate_adapters(wgpu::Backends::all());
        if adapters.is_empty() {
            return Err("no GPU adapters enumerated".into());
        }
        let adapter = adapters
            .into_iter()
            .filter(|a| {
                let info = a.get_info();
                match sel.as_str() {
                    "auto" | "" => true,
                    "dgpu" => info.device_type == wgpu::DeviceType::DiscreteGpu,
                    "igpu" => info.device_type == wgpu::DeviceType::IntegratedGpu,
                    other => info.name.to_lowercase().contains(other),
                }
            })
            .max_by_key(|a| {
                let info = a.get_info();
                let class = match info.device_type {
                    wgpu::DeviceType::DiscreteGpu => 3,
                    wgpu::DeviceType::IntegratedGpu => 2,
                    _ => 0,
                };
                let backend = match info.backend {
                    wgpu::Backend::Vulkan => 2,
                    wgpu::Backend::Dx12 => 1,
                    _ => 0,
                };
                class * 10 + backend
            })
            .ok_or_else(|| format!("no GPU adapter matches --gpu {selector}"))?;
        let info = adapter.get_info();

        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None))
                .map_err(|e| format!("GPU device request failed: {e}"))?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cave-carve"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()),
        });

        let (perms, octs, amps, ranges) = flatten_noises(gen);
        let mk_storage = |label: &str, bytes: &[u8]| {
            use wgpu::util::DeviceExt;
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytes,
                usage: wgpu::BufferUsages::STORAGE,
            })
        };
        let perms_buf = mk_storage("perms", bytemuck::cast_slice(&perms));
        let octs_buf = mk_storage("octs", bytemuck::cast_slice(&octs));
        let amps_buf = mk_storage("amps", bytemuck::cast_slice(&amps));
        let ranges_buf = mk_storage("ranges", bytemuck::cast_slice(&ranges));

        let entries: Vec<wgpu::BindGroupLayoutEntry> = (0..8)
            .map(|i| wgpu::BindGroupLayoutEntry {
                binding: i,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: if i == 0 {
                        wgpu::BufferBindingType::Uniform
                    } else {
                        wgpu::BufferBindingType::Storage {
                            read_only: (1..=4).contains(&i),
                        }
                    },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            })
            .collect();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("carve-layout"),
            entries: &entries,
        });
        let pipe_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let mk_pipe = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipe_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let corners_pipe = mk_pipe("corners");
        let blocks_pipe = mk_pipe("blocks");

        Ok(GpuCarver {
            device,
            queue,
            corners_pipe,
            blocks_pipe,
            layout,
            perms_buf,
            octs_buf,
            amps_buf,
            ranges_buf,
            adapter_name: format!("{} ({:?})", info.name, info.backend),
            seed,
        })
    }

    /// Compute the carve mask for one region rect. Returns one bit per block in
    /// `(column, y)` order: `idx = (y - y_lo) * w * h + col`, `col = (x - min_x) * h
    /// + (z - min_z)` - matching the CPU loop's coordinate conventions.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn carve_mask(
        &self,
        min_x: i32,
        max_x: i32,
        min_z: i32,
        max_z: i32,
        y_lo: i32,
        y_hi: i32,
        cy_lo: i32,
        cy_hi: i32,
        cheese_k: f32,
        top_gate: i32,
        surf: &[i32],
    ) -> Result<Vec<u32>, String> {
        let started = std::time::Instant::now();
        let w = (max_x - min_x + 1) as u32;
        let h = (max_z - min_z + 1) as u32;
        let cx0 = min_x.div_euclid(4);
        let cz0 = min_z.div_euclid(4);
        let cx1 = max_x.div_euclid(4);
        let cz1 = max_z.div_euclid(4);
        let nx = (cx1 - cx0 + 2) as u32;
        let nz = (cz1 - cz0 + 2) as u32;
        let ny = (cy_hi - cy_lo + 2) as u32;
        let nyb = (y_hi - y_lo + 1).max(0) as u32;
        if nyb == 0 {
            return Ok(Vec::new());
        }

        let params = Params {
            min_x,
            min_z,
            w,
            h,
            cx0,
            cz0,
            nx,
            nz,
            cy_lo,
            ny,
            y_lo,
            nyb,
            cheese_k,
            top_gate,
            _pad0: 0,
            _pad1: 0,
        };

        use wgpu::util::DeviceExt;
        let params_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let surf_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("surf"),
                contents: bytemuck::cast_slice(surf),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let corners_len = (nx * nz * ny) as u64 * 4;
        let corners_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("corners"),
            size: corners_len,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let mask_words = (w as u64 * h as u64 * nyb as u64).div_ceil(32);
        let mask_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mask"),
            size: mask_words * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let read_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mask-read"),
            size: mask_words * 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.perms_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.octs_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.amps_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.ranges_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: surf_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: corners_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: mask_buf.as_entire_binding(),
                },
            ],
        });

        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_bind_group(0, &bind, &[]);
            pass.set_pipeline(&self.corners_pipe);
            let corner_groups = (nx * nz * ny).div_ceil(64);
            pass.dispatch_workgroups(corner_groups, 1, 1);
            pass.set_pipeline(&self.blocks_pipe);
            let col_groups = (w * h).div_ceil(256);
            pass.dispatch_workgroups(col_groups, nyb, 1);
        }
        enc.copy_buffer_to_buffer(&mask_buf, 0, &read_buf, 0, mask_words * 4);
        self.queue.submit([enc.finish()]);

        let slice = read_buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv()
            .map_err(|_| "GPU readback channel closed".to_string())?
            .map_err(|e| format!("GPU readback failed: {e:?}"))?;
        let words: Vec<u32> = bytemuck::cast_slice(&slice.get_mapped_range()).to_vec();
        read_buf.unmap();
        GPU_BUSY_MS.fetch_add(started.elapsed().as_millis() as u64, Ordering::Relaxed);
        Ok(words)
    }
}

static CARVER: OnceLock<Option<GpuCarver>> = OnceLock::new();

/// The process-wide carver, built on first use from `--gpu` / `ARNIS_GPU`.
///
/// `None` means "run on the CPU" - either because the flag is off, or because
/// device creation failed (which is logged ONCE, not per region). Init measured
/// ~130 ms on the iGPU and ~790 ms waking the dGPU, amortised across the run.
pub(crate) fn carver_for(selector: &str, gen: &CaveGen, seed: i64) -> Option<&'static GpuCarver> {
    let carver = CARVER.get_or_init(|| {
        if selector.is_empty() || selector == "off" {
            return None;
        }
        match GpuCarver::create(selector, gen, seed) {
            Ok(c) => {
                println!("    Cave GPU: {}", c.adapter_name);
                Some(c)
            }
            Err(e) => {
                eprintln!("    Cave GPU unavailable ({e}); using the CPU path.");
                None
            }
        }
    });
    match carver {
        // A second seed in one process would need a second noise upload; nothing does
        // that today, so fall back to CPU rather than silently using the wrong seed.
        Some(c) if c.seed == seed => Some(c),
        _ => None,
    }
}

#[cfg(test)]
mod gpu_parity_tests {
    use super::*;

    /// The contract: GPU and CPU agree on >99.9% of blocks. Not bit-equality - the
    /// GPU is f32 against the CPU's f64, and near the carve threshold that flips the
    /// odd cell. What this catches is STRUCTURAL error: a wrong perm table, a
    /// misindexed octave, a transcription slip in one of the density functions - any
    /// of which disagree on whole regions, not fractions of a percent.
    ///
    /// Skips (passes vacuously) when no GPU adapter exists, so CI without a GPU
    /// stays green; the point of running it locally is the number it prints.
    #[test]
    fn gpu_mask_matches_cpu_reference() {
        let seed = 424242i64;
        let gen = CaveGen::new(seed);
        let carver = match GpuCarver::create("auto", &gen, seed) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skipping GPU parity test: {e}");
                return;
            }
        };
        eprintln!("parity adapter: {}", carver.adapter_name);

        // One 128x128 rect, flat surface at 80: high enough for the full band.
        let (min_x, max_x, min_z, max_z) = (256i32, 383i32, -128i32, -1i32);
        let (w, h) = (128usize, 128usize);
        let surf = vec![80i32; w * h];
        let top_gate: i32 = 6;
        let y_lo: i32 = -63;
        let y_hi: i32 = 80 - top_gate;
        let cy_lo = y_lo.div_euclid(8);
        let cy_hi = y_hi.div_euclid(8);
        let (_, cheese_k) = gen.gpu_export();

        let words = carver
            .carve_mask(
                min_x,
                max_x,
                min_z,
                max_z,
                y_lo,
                y_hi,
                cy_lo,
                cy_hi,
                cheese_k as f32,
                top_gate,
                &surf,
            )
            .expect("GPU dispatch");
        let cols = (w * h) as u64;
        let mut gpu_set = std::collections::HashSet::new();
        for (wi, &word) in words.iter().enumerate() {
            let mut bits = word;
            while bits != 0 {
                let b = bits.trailing_zeros() as u64;
                bits &= bits - 1;
                let idx = wi as u64 * 32 + b;
                let by = y_lo + (idx / cols) as i32;
                let col = idx % cols;
                let bx = min_x + (col / h as u64) as i32;
                let bz = min_z + (col % h as u64) as i32;
                gpu_set.insert((bx, by, bz));
            }
        }

        // CPU reference: the same cell-corner + trilerp + noodle loop carve_region runs.
        let mut cpu_set = std::collections::HashSet::new();
        let mut total_blocks = 0u64;
        for cx in min_x.div_euclid(4)..=max_x.div_euclid(4) {
            for cz in min_z.div_euclid(4)..=max_z.div_euclid(4) {
                let (wx0, wx1) = (cx * 4, cx * 4 + 4);
                let (wz0, wz1) = (cz * 4, cz * 4 + 4);
                for cy in cy_lo..=cy_hi {
                    let (wy0, wy1) = (cy * 8, cy * 8 + 8);
                    let c = |x: i32, y: i32, z: i32| gen.combined_density(x, y, z);
                    let n000 = c(wx0, wy0, wz0);
                    let n100 = c(wx1, wy0, wz0);
                    let n010 = c(wx0, wy1, wz0);
                    let n110 = c(wx1, wy1, wz0);
                    let n001 = c(wx0, wy0, wz1);
                    let n101 = c(wx1, wy0, wz1);
                    let n011 = c(wx0, wy1, wz1);
                    let n111 = c(wx1, wy1, wz1);
                    let l = |t: f64, a: f64, b: f64| a + t * (b - a);
                    for by in wy0.max(y_lo)..=(wy1 - 1).min(y_hi) {
                        let fy = (by - wy0) as f64 / 8.0;
                        let xz00 = l(fy, n000, n010);
                        let xz10 = l(fy, n100, n110);
                        let xz01 = l(fy, n001, n011);
                        let xz11 = l(fy, n101, n111);
                        for bx in wx0.max(min_x)..wx1.min(max_x + 1) {
                            let fx = (bx - wx0) as f64 / 4.0;
                            let z0v = l(fx, xz00, xz10);
                            let z1v = l(fx, xz01, xz11);
                            for bz in wz0.max(min_z)..wz1.min(max_z + 1) {
                                total_blocks += 1;
                                let fz = (bz - wz0) as f64 / 4.0;
                                let combined = l(fz, z0v, z1v);
                                let carve =
                                    combined <= 0.0 || gen.noodle_density(bx, by, bz) <= 0.0;
                                if carve {
                                    cpu_set.insert((bx, by, bz));
                                }
                            }
                        }
                    }
                }
            }
        }

        let disagree = gpu_set.symmetric_difference(&cpu_set).count() as u64;
        let carve_rate = cpu_set.len() as f64 / total_blocks as f64 * 100.0;
        let disagree_pct = disagree as f64 / total_blocks as f64 * 100.0;
        eprintln!(
            "parity: {total_blocks} blocks | CPU carves {} ({carve_rate:.2}%) | GPU carves {} | disagree {disagree} ({disagree_pct:.4}%)",
            cpu_set.len(),
            gpu_set.len(),
        );
        assert!(
            disagree_pct < 0.1,
            "GPU carve mask disagrees with the CPU on {disagree_pct:.4}% of blocks -              that is structural, not float noise"
        );
        assert!(
            !cpu_set.is_empty() && !gpu_set.is_empty(),
            "one side carved nothing; the comparison is vacuous"
        );
    }
}
