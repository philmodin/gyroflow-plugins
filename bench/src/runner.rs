use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::sync::{atomic::AtomicBool, Arc};
use std::time::Instant;

use gyroflow_plugin_base::gyroflow_core::{
    self,
    gpu::{BufferDescription, BufferSource, Buffers},
    stabilization::{RGBA8, RGBA16, RGBAf, RGBAf16},
    StabilizationManager,
};

use crate::cli::{BackendArg, PixelFormatArg, RunArgs};
use crate::result::{Cell, IterationData, Summary, summarize};
use crate::source::FrameSource;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PixelFormat { Rgba8, Rgba16, Rgbaf16, Rgbaf }

impl PixelFormat {
    fn name(self) -> &'static str {
        match self { Self::Rgba8 => "rgba8", Self::Rgba16 => "rgba16", Self::Rgbaf16 => "rgbaf16", Self::Rgbaf => "rgbaf" }
    }
    fn bpp(self) -> usize {
        match self { Self::Rgba8 => 4, Self::Rgba16 => 8, Self::Rgbaf16 => 8, Self::Rgbaf => 16 }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Backend { Cpu, Opencl, Metal, Cuda }

impl Backend {
    fn name(self) -> &'static str {
        match self { Self::Cpu => "cpu", Self::Opencl => "opencl", Self::Metal => "metal", Self::Cuda => "cuda" }
    }
}

pub fn resolve_pixel_formats(args: &[PixelFormatArg]) -> Vec<PixelFormat> {
    if args.iter().any(|a| matches!(a, PixelFormatArg::All)) {
        return vec![PixelFormat::Rgba8, PixelFormat::Rgba16, PixelFormat::Rgbaf16, PixelFormat::Rgbaf];
    }
    let mut out = Vec::new();
    for a in args {
        let p = match a {
            PixelFormatArg::Rgba8 => PixelFormat::Rgba8,
            PixelFormatArg::Rgba16 => PixelFormat::Rgba16,
            PixelFormatArg::Rgbaf16 => PixelFormat::Rgbaf16,
            PixelFormatArg::Rgbaf => PixelFormat::Rgbaf,
            PixelFormatArg::All => unreachable!(),
        };
        if !out.contains(&p) { out.push(p); }
    }
    out
}

pub fn resolve_backends(args: &[BackendArg]) -> Vec<Backend> {
    let want_all = args.iter().any(|a| matches!(a, BackendArg::All));
    let mut out = Vec::new();
    let push = |b: Backend, out: &mut Vec<Backend>| { if !out.contains(&b) { out.push(b); } };
    if want_all {
        push(Backend::Cpu, &mut out);
        #[cfg(any(target_os = "macos", target_os = "ios"))] push(Backend::Metal, &mut out);
        push(Backend::Opencl, &mut out);
        #[cfg(any(target_os = "windows", target_os = "linux"))] push(Backend::Cuda, &mut out);
        return out;
    }
    for a in args {
        let b = match a {
            BackendArg::Cpu => Backend::Cpu,
            BackendArg::Opencl => Backend::Opencl,
            BackendArg::Metal => Backend::Metal,
            BackendArg::Cuda => Backend::Cuda,
            BackendArg::All => unreachable!(),
        };
        push(b, &mut out);
    }
    out
}

/// Build a StabilizationManager from a .gyroflow project file. Mirrors the
/// canonical setup in common/src/lib.rs:537 (`stab_manager`) for the
/// .gyroflow-project branch.
///
/// `share_wgpu_instances` is **off** here (the plugin sets it on) so that
/// gyroflow-core caches its OpenCL/wgpu wrappers on the Stabilization
/// instance (`self.cl`, `self.wgpu`) instead of thread-globally
/// (`CACHED_OPENCL`, `CACHED_WGPU`). That keeps multiple cells in one bench
/// process from contaminating each other's backend selection.
///
/// If `size_override` is set, both input and output sizes are forced to that
/// value (handy for sweeping sizes); otherwise the project's recorded sizes
/// are used as-is.
pub fn build_manager(project_path: &Path, size_override: Option<(usize, usize)>) -> Result<Arc<StabilizationManager>> {
    let project_data = std::fs::read_to_string(project_path)
        .with_context(|| format!("failed to read project file {}", project_path.display()))?;
    let url = gyroflow_core::filesystem::path_to_url(&project_path.to_string_lossy());

    let stab = StabilizationManager::default();
    {
        let mut s = stab.stabilization.write();
        s.share_wgpu_instances = false;
    }
    let mut is_preset = false;
    stab.import_gyroflow_data(
        project_data.as_bytes(),
        true,
        Some(&url),
        |_| {},
        Arc::new(AtomicBool::new(false)),
        &mut is_preset,
        true,
    ).map_err(|e| anyhow!("import_gyroflow_data failed: {e:?}"))?;

    {
        let mut p = stab.params.write();
        p.framebuffer_inverted = false;
    }
    stab.init_size();
    if let Some((w, h)) = size_override {
        stab.params.write().size = (w, h);
        stab.set_output_size(w, h);
    }
    stab.invalidate_smoothing();
    stab.recompute_blocking();
    {
        let inverse = true;
        let kf = stab.keyframes.read();
        stab.params.write().calculate_ramped_timestamps(&kf, inverse, inverse);
    }

    Ok(Arc::new(stab))
}

/// Cell-relevant params snapshot from the StabilizationManager.
#[derive(Copy, Clone, Debug)]
pub struct ProjectShape {
    pub input_size: (usize, usize),
    pub output_size: (usize, usize),
    pub frame_count: usize,
    pub fps: f64,
    pub duration_us: i64,
}

pub fn read_shape(stab: &Arc<StabilizationManager>) -> ProjectShape {
    let p = stab.params.read();
    ProjectShape {
        input_size: p.size,
        output_size: p.output_size,
        frame_count: p.frame_count,
        fps: if p.fps > 0.0 { p.fps } else { 30.0 },
        duration_us: (p.duration_ms * 1000.0) as i64,
    }
}

/// Tell gyroflow-core which device to use for the next render. Returns the
/// device-list entry that was selected, or an error if no matching device exists.
fn select_device(stab: &Arc<StabilizationManager>, backend: Backend) -> Result<String> {
    let devices = stab.stabilization.read().list_devices();
    // GPU_LIST is the global Stabilization::set_device reads from to map an
    // index → device. The plugin populates it via StabilizationManager::list_gpu_devices()
    // (threaded with a callback). We do it inline.
    *gyroflow_plugin_base::gyroflow_core::stabilization::GPU_LIST.write() = devices.clone();
    let (idx, label): (isize, String) = match backend {
        Backend::Cpu => (-1, "cpu (software)".to_string()),
        Backend::Opencl => {
            let i = devices.iter().position(|d| d.starts_with("[OpenCL]"))
                .ok_or_else(|| anyhow!("no OpenCL device available on this host"))?;
            (i as isize, devices[i].clone())
        }
        Backend::Metal => {
            #[cfg(not(any(target_os = "macos", target_os = "ios")))]
            { return Err(anyhow!("Metal backend only available on macOS/iOS")); }
            #[cfg(any(target_os = "macos", target_os = "ios"))]
            {
                let i = devices.iter().position(|d| d.starts_with("[wgpu]"))
                    .ok_or_else(|| anyhow!("no wgpu/Metal device available on this host"))?;
                (i as isize, devices[i].clone())
            }
        }
        Backend::Cuda => {
            // The CUDA path in gyroflow-core only fires for BufferSource::CUDABuffer
            // (zero-copy GPU buffer). It cannot be selected via set_device with a
            // CPU buffer, so we can't measure it without standalone CUDA setup.
            return Err(anyhow!(
                "CUDA backend requires zero-copy CUDABuffer setup; not implemented standalone"
            ));
        }
    };
    stab.stabilization.write().set_device(idx);
    Ok(label)
}

/// What the run is actually going to bench, after merging CLI overrides with
/// project defaults. Reported to the user before measurement so it's obvious
/// what was picked.
pub struct EffectiveConfig {
    pub size_override: Option<(usize, usize)>,
    pub input_size: (usize, usize),
    pub output_size: (usize, usize),
    pub frames: usize,
    pub warmup: usize,
    pub iterations: usize,
    pub fps: f64,
}

pub fn resolve_config(args: &RunArgs) -> Result<EffectiveConfig> {
    // Build a throwaway manager just to read the project's recorded shape,
    // so we can pick frame counts / sizes before the real per-cell rebuilds.
    let size_override = match (args.width, args.height) {
        (Some(w), Some(h)) => Some((w, h)),
        (None, None) => None,
        _ => return Err(anyhow!("--width and --height must be set together")),
    };
    let probe = build_manager(&args.project, size_override)?;
    let shape = read_shape(&probe);
    drop(probe);

    let frames = args.frames.unwrap_or(shape.frame_count.max(1));
    Ok(EffectiveConfig {
        size_override,
        input_size: shape.input_size,
        output_size: shape.output_size,
        frames,
        warmup: args.warmup,
        iterations: args.iterations,
        fps: shape.fps,
    })
}

pub fn run(args: &RunArgs, eff: &EffectiveConfig) -> Result<Vec<Cell>> {
    let backends = resolve_backends(&args.backend);
    let formats = resolve_pixel_formats(&args.pixel_format);
    if backends.is_empty() { return Err(anyhow!("no backends selected")); }
    if formats.is_empty() { return Err(anyhow!("no pixel formats selected")); }

    let mut cells = Vec::new();
    for backend in &backends {
        for fmt in &formats {
            println!("==> {} / {}", backend.name(), fmt.name());
            // Rebuild the manager per cell so per-instance backend caches
            // (self.cl, self.wgpu) start empty. Avoids one cell's backend
            // sticking to the next cell's measurements.
            let stab = build_manager(&args.project, eff.size_override)?;
            let shape = read_shape(&stab);
            let frame_step_us = (1_000_000.0 / shape.fps) as i64;

            let cell = match select_device(&stab, *backend) {
                Ok(label) => {
                    println!("    device: {}", label);
                    run_cell(&stab, *backend, *fmt, args, eff, &shape, frame_step_us)?
                }
                Err(e) => {
                    let msg = format!("{}", e);
                    println!("    SKIP: {}", msg);
                    skip_cell(*backend, *fmt, &msg)
                }
            };
            print_cell_summary(&cell);
            cells.push(cell);
        }
    }
    Ok(cells)
}

fn skip_cell(backend: Backend, fmt: PixelFormat, msg: &str) -> Cell {
    Cell {
        backend: backend.name().into(),
        pixel_format: fmt.name().into(),
        iterations: vec![],
        summary: Summary { mean_us: 0.0, median_us: 0.0, p95_us: 0.0, p99_us: 0.0, stddev_us: 0.0, min_us: 0, max_us: 0, samples: 0 },
        actual_backend: None,
        note: Some(msg.into()),
    }
}

fn run_cell(
    stab: &Arc<StabilizationManager>,
    backend: Backend,
    fmt: PixelFormat,
    args: &RunArgs,
    eff: &EffectiveConfig,
    shape: &ProjectShape,
    frame_step_us: i64,
) -> Result<Cell> {
    let bpp = fmt.bpp();
    let (in_w, in_h) = shape.input_size;
    let (out_w, out_h) = shape.output_size;
    let in_stride = in_w * bpp;
    let out_stride = out_w * bpp;
    let in_bytes = in_stride * in_h;
    let out_bytes = out_stride * out_h;

    let mut input = vec![0u8; in_bytes];
    let mut output = vec![0u8; out_bytes];

    let mut source = if let Some(video) = &args.video {
        if !matches!(fmt, PixelFormat::Rgba8) {
            return Ok(skip_cell(backend, fmt, "video source only supports rgba8 in v1"));
        }
        FrameSource::video(video, in_w, in_h)?
    } else {
        FrameSource::synthetic(in_bytes)
    };

    // Sanity-check: one untimed frame must succeed before we start measuring.
    source.fill(&mut input)?;
    let ts0 = pick_ts(0, frame_step_us, shape.duration_us);
    let actual_backend = process_one(stab, fmt, (in_w, in_h, in_stride), (out_w, out_h, out_stride), ts0, &mut input, &mut output)
        .with_context(|| "sanity-check process_pixels failed; bad project / size mismatch?")?;
    let actual_backend_label = if actual_backend.is_empty() { "cpu-software".to_string() } else { actual_backend.to_string() };
    println!("    actual backend: {}", actual_backend_label);

    // Warmup
    for f in 0..eff.warmup {
        source.fill(&mut input)?;
        let ts = pick_ts(f as i64, frame_step_us, shape.duration_us);
        process_one(stab, fmt, (in_w, in_h, in_stride), (out_w, out_h, out_stride), ts, &mut input, &mut output)?;
    }

    let mut iterations = Vec::with_capacity(eff.iterations);
    let mut all = Vec::with_capacity(eff.iterations * eff.frames);
    for it in 0..eff.iterations {
        let mut frame_us = Vec::with_capacity(eff.frames);
        for f in 0..eff.frames {
            source.fill(&mut input)?;
            let ts = pick_ts(f as i64, frame_step_us, shape.duration_us);
            let t0 = Instant::now();
            process_one(stab, fmt, (in_w, in_h, in_stride), (out_w, out_h, out_stride), ts, &mut input, &mut output)?;
            let dt = t0.elapsed().as_micros() as u64;
            frame_us.push(dt);
        }
        let s = summarize(&frame_us);
        println!("    iter {}: median {:.1} us, p95 {:.1} us, n={}", it, s.median_us, s.p95_us, s.samples);
        all.extend_from_slice(&frame_us);
        iterations.push(IterationData { frame_us });
    }

    Ok(Cell {
        backend: backend.name().into(),
        pixel_format: fmt.name().into(),
        iterations,
        summary: summarize(&all),
        actual_backend: Some(actual_backend_label),
        note: None,
    })
}

fn pick_ts(frame_idx: i64, step_us: i64, duration_us: i64) -> i64 {
    if duration_us <= 0 { return frame_idx * step_us; }
    (frame_idx * step_us).rem_euclid(duration_us)
}

fn process_one(
    stab: &Arc<StabilizationManager>,
    fmt: PixelFormat,
    in_size: (usize, usize, usize),
    out_size: (usize, usize, usize),
    ts_us: i64,
    input: &mut [u8],
    output: &mut [u8],
) -> Result<&'static str> {
    let mut buffers = Buffers {
        input: BufferDescription {
            size: in_size,
            rect: None,
            data: BufferSource::Cpu { buffer: input },
            rotation: None,
            texture_copy: false,
        },
        output: BufferDescription {
            size: out_size,
            rect: None,
            data: BufferSource::Cpu { buffer: output },
            rotation: None,
            texture_copy: false,
        },
    };
    let r = match fmt {
        PixelFormat::Rgba8   => stab.process_pixels::<RGBA8>(ts_us, None, &mut buffers),
        PixelFormat::Rgba16  => stab.process_pixels::<RGBA16>(ts_us, None, &mut buffers),
        PixelFormat::Rgbaf16 => stab.process_pixels::<RGBAf16>(ts_us, None, &mut buffers),
        PixelFormat::Rgbaf   => stab.process_pixels::<RGBAf>(ts_us, None, &mut buffers),
    };
    r.map(|info| info.backend).map_err(|e| anyhow!("process_pixels error: {e:?}"))
}

fn print_cell_summary(cell: &Cell) {
    if let Some(note) = &cell.note {
        println!("    SKIP: {}", note);
        return;
    }
    let s = &cell.summary;
    println!(
        "    summary: mean {:.1} us, median {:.1} us, p95 {:.1} us, p99 {:.1} us, n={}",
        s.mean_us, s.median_us, s.p95_us, s.p99_us, s.samples
    );
}
