use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "gyroflow-bench", about = "Benchmark the Gyroflow stabilization hot path", version)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Run a benchmark and write a JSON result file.
    Run(RunArgs),
    /// Compare two saved runs cell-by-cell.
    Compare(CompareArgs),
    /// List saved runs.
    List(ListArgs),
}

#[derive(clap::Args, Debug)]
pub struct RunArgs {
    /// Run name (required, must be unique). Result file: <name>.bench.json
    #[arg(long)]
    pub name: String,

    /// Path to the .gyroflow project file.
    #[arg(long)]
    pub project: PathBuf,

    /// Override the project's frame size, format WxH (applies to input + output).
    #[arg(long)]
    pub resolution: Option<String>,

    /// Pixel format(s); comma-separated, or "all".
    #[arg(long, value_delimiter = ',', default_values_t = vec![PixelFormatArg::Rgba8])]
    pub pixel_format: Vec<PixelFormatArg>,

    /// Frames to time per iteration. Defaults to the project's frame_count.
    #[arg(long)]
    pub frames: Option<usize>,

    /// Untimed warmup frames before timing.
    #[arg(long, default_value_t = 30)]
    pub warmup: usize,

    /// Number of independent iterations.
    #[arg(long, default_value_t = 3)]
    pub iterations: usize,

    /// Backend(s); comma-separated, or "all". Default: GPU backends only (cpu is skipped
    /// because it is the slow path). Pass `--backend cpu` (or `--backend all`) to include it.
    #[arg(long, value_delimiter = ',')]
    pub backend: Vec<BackendArg>,

    /// Optional path to a video file to decode via ffmpeg (rgba8 only).
    #[arg(long)]
    pub video: Option<PathBuf>,

    /// Output directory for the result JSON (default: <gyroflow data dir>/benchmarks).
    #[arg(long)]
    pub output: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
pub struct CompareArgs {
    /// Path to a result file, or a name to look up under --dir.
    pub baseline: String,
    /// Path to a result file, or a name to look up under --dir.
    pub candidate: String,
    /// Directory to search when args aren't absolute paths.
    /// Default: <gyroflow data dir>/benchmarks.
    #[arg(long)]
    pub dir: Option<PathBuf>,
    /// Regression threshold percent.
    #[arg(long, default_value_t = 5.0)]
    pub threshold: f64,
}

#[derive(clap::Args, Debug)]
pub struct ListArgs {
    /// Directory to list. Default: <gyroflow data dir>/benchmarks.
    pub dir: Option<PathBuf>,
}

#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq, Hash)]
pub enum PixelFormatArg {
    Rgba8,
    Rgba16,
    Rgbaf16,
    Rgbaf,
    All,
}

impl std::fmt::Display for PixelFormatArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PixelFormatArg::Rgba8 => "rgba8",
            PixelFormatArg::Rgba16 => "rgba16",
            PixelFormatArg::Rgbaf16 => "rgbaf16",
            PixelFormatArg::Rgbaf => "rgbaf",
            PixelFormatArg::All => "all",
        })
    }
}

#[derive(Copy, Clone, Debug, ValueEnum, PartialEq, Eq, Hash)]
pub enum BackendArg {
    Cpu,
    Opencl,
    Metal,
    Cuda,
    All,
}

impl std::fmt::Display for BackendArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            BackendArg::Cpu => "cpu",
            BackendArg::Opencl => "opencl",
            BackendArg::Metal => "metal",
            BackendArg::Cuda => "cuda",
            BackendArg::All => "all",
        })
    }
}
