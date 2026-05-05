# gyroflow-bench

Benchmark harness for the Gyroflow stabilization hot path
(`StabilizationManager::process_pixels`). Bypasses the OFX host so timings are
not contaminated by Resolve / Nuke / Fusion overhead, and pulls the same
`gyroflow-core` revision the plugin is built against (via
`gyroflow-plugin-base` → `common/Cargo.toml`).

## Build

```
cargo build --release -p gyroflow-bench
```

## Run

```
cargo run --release -p gyroflow-bench -- run \
    --project /path/to/project.gyroflow \
    --label baseline
```

By default the input size, output size, and frame count come from the
project itself. Override with `--width WxH-pair`, `--height ...`, or
`--frames N` if you need a specific workload (e.g. for a sweep).
Warmup defaults to 30 frames, iterations to 3.

Result JSON is written to `bench/results/` (gitignored).

`--backend all` (default) tries every backend available on this platform.
The mapping:

| `--backend` | how it's selected | dispatched as |
|-------------|-------------------|---------------|
| `cpu`       | `Stabilization::set_device(-1)` | pure CPU software (`actual_backend = "CPU"`) |
| `opencl`    | first `[OpenCL]` device from gyroflow-core's device list | OpenCL via `OclWrapper`, CPU↔GPU copies on each frame (`actual_backend = "OpenCL"`) |
| `metal`     | first `[wgpu]` device on macOS (wgpu uses Metal under the hood) | wgpu/Metal, CPU↔GPU copies on each frame (`actual_backend = "wgpu"`) |
| `cuda`      | not implemented standalone — gyroflow-core's CUDA path requires zero-copy `BufferSource::CUDABuffer` | skipped with a note |

Each cell rebuilds a fresh `StabilizationManager` and uses
`share_wgpu_instances = false` so per-cell GPU caches (`self.cl`, `self.wgpu`)
don't leak across cells.

The cell records `actual_backend` (the backend gyroflow-core actually
dispatched to) alongside the requested `backend`. They should match — if not,
investigate before trusting the numbers.

Note: OpenCL and Metal cells measure **GPU compute + per-frame CPU↔GPU
copies**, which is what the OFX host path does when it hands the plugin a CPU
buffer. True zero-copy GPU-resident benchmarks (`BufferSource::OpenCL{texture}`,
`BufferSource::MetalBuffer`) would require allocating GPU memory directly and
are not implemented.

`--pixel-format all` runs every supported format.

`--video <path>` decodes a real video via `ffmpeg -f rawvideo -pix_fmt rgba`
instead of timing on a synthetic in-memory buffer (rgba8 only in v1; mixes
decode/IO time into the measurement).

## Compare

```
cargo run --release -p gyroflow-bench -- compare baseline perf-branch
```

Each argument resolves to a result file: tried as a path first, then as an
exact match against the run's `--label`, then as a filename substring under
`--dir` (default `bench/results`). If multiple files match, the newest is
picked. You can still pass full paths if you want.

Per `(backend, pixel_format)` cell prints baseline median, candidate median,
percent delta, and a 95% bootstrap CI (1000 resamples) of the percent change.
A cell is flagged `REGRESSION` when delta exceeds the threshold (default 5%)
*and* the CI lower bound is positive; flagged `WIN` symmetrically; otherwise
`noise`.

## List

```
cargo run --release -p gyroflow-bench -- list [bench/results]
```

## Workflow for a perf branch

```
git checkout main
cargo run --release -p gyroflow-bench -- run --project p.gyroflow --label main

git checkout perf-branch
cargo run --release -p gyroflow-bench -- run --project p.gyroflow --label perf

cargo run --release -p gyroflow-bench -- compare \
    bench/results/<main>.json bench/results/<perf>.json
```

## Verifying the tool

1. **Noise floor:** run twice on the same commit, compare — every cell should
   be `noise` with `|delta| < ~3%`.
2. **Known-slower check:** locally insert `std::thread::sleep(...)` in
   `runner::process_one`, rerun, compare — expect `REGRESSION`.

## Caveats

- The `.gyroflow` project may reference an input video file by absolute path.
  `import_gyroflow_data` may need that file present for size/FPS metadata even
  in synthetic mode. If load fails, check that the referenced video exists.
- `mimalloc` is the global allocator (inherited from `gyroflow-plugin-base`),
  matching what the plugin uses at runtime.
