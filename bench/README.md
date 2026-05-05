# gyroflow-bench (plugin)

Benchmark harness for the Gyroflow stabilization hot path
(`StabilizationManager::process_pixels`) as the OFX plugin uses it. Bypasses
the OFX host so timings are not contaminated by Resolve / Nuke / Fusion
overhead, and pulls the same `gyroflow-core` revision the plugin is built
against.

CLI is intentionally aligned with the standalone harness in the `gyroflow`
repo (`gyroflow-bench` under `src/core/`) so the two tools share flag
names, the run-naming scheme, and the on-disk results location.

## Build

```
cargo build --release -p gyroflow-bench
```

## Run

```
cargo run --release -p gyroflow-bench -- run \
    --name baseline \
    --project /path/to/project.gyroflow
```

Required:
- `--name <NAME>` — unique run name. Result file: `<NAME>.bench.json`.
- `--project <PATH>` — path to a `.gyroflow` project (supplied externally;
  not stored in this repo).

Optional overrides:
- `--resolution WxH` — override the project's input + output frame size.
- `--frames N` — frames per iteration (default: project's frame count).
- `--warmup N` — untimed warmup frames (default: 30).
- `--iterations N` — independent timed iterations (default: 3).
- `--pixel-format rgba8|rgba16|rgbaf16|rgbaf|all` — comma-separated, default `rgba8`.
- `--backend cpu|opencl|metal|cuda|all` — comma-separated. **Default: GPU
  backends only** (CPU is the slow path and is skipped unless explicitly
  requested with `--backend cpu` or `--backend all`).
- `--video <path>` — decode a real video via ffmpeg instead of synthetic
  buffers (rgba8 only in v1).
- `--output <DIR>` — override the result directory.

Result JSON is written to `<gyroflow data dir>/benchmarks/<name>.bench.json`
by default — outside the repo, matching the standalone harness.

| `--backend` | how it's selected | dispatched as |
|-------------|-------------------|---------------|
| `cpu`       | `Stabilization::set_device(-1)` | pure CPU software (slow path) |
| `opencl`    | first `[OpenCL]` device | OpenCL via `OclWrapper`, CPU↔GPU copies per frame |
| `metal`     | first `[wgpu]` device on macOS | wgpu/Metal, CPU↔GPU copies per frame |
| `cuda`      | not implemented standalone (needs zero-copy `BufferSource::CUDABuffer`) | skipped |

Each cell rebuilds a fresh `StabilizationManager` so per-cell GPU caches
don't leak across cells.

## Compare

```
cargo run --release -p gyroflow-bench -- compare baseline perf
```

Each argument is treated as a run name first (`<dir>/<name>.bench.json`),
falling back to a path. `--dir` defaults to `<gyroflow data dir>/benchmarks`.

Per `(backend, pixel_format)` cell prints baseline median, candidate median,
percent delta, and a 95% bootstrap CI of the percent change. A cell is
flagged `REGRESSION` when delta exceeds the threshold (default 5%) *and*
the CI lower bound is positive; `WIN` symmetrically; otherwise `noise`.

## List

```
cargo run --release -p gyroflow-bench -- list
```

Lists every `*.bench.json` in `<gyroflow data dir>/benchmarks` (or the
directory passed positionally).

## Workflow for a perf branch

```
git checkout main
cargo run --release -p gyroflow-bench -- run --name main --project p.gyroflow

git checkout perf-branch
cargo run --release -p gyroflow-bench -- run --name perf --project p.gyroflow

cargo run --release -p gyroflow-bench -- compare main perf
```

## Caveats

- The `.gyroflow` project may reference an input video file by absolute path.
  `import_gyroflow_data` may need that file present for size/FPS metadata
  even in synthetic mode.
- OpenCL/Metal cells measure GPU compute + per-frame CPU↔GPU copies,
  matching what the OFX host path does.
- `mimalloc` is the global allocator (inherited from `gyroflow-plugin-base`),
  matching what the plugin uses at runtime.
