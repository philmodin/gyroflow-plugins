use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct BenchResult {
    pub schema_version: u32,
    pub timestamp: String,
    pub label: String,
    pub host: HostInfo,
    pub git: GitInfo,
    pub config: RunConfig,
    pub cells: Vec<Cell>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HostInfo {
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub cpu_model: String,
    pub cpu_cores: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GitInfo {
    pub plugin_rev: String,
    pub core_rev: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RunConfig {
    pub project: String,
    pub input_size: [usize; 2],
    pub output_size: [usize; 2],
    pub frames: usize,
    pub warmup: usize,
    pub iterations: usize,
    pub frame_source: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Cell {
    pub backend: String,
    pub pixel_format: String,
    pub iterations: Vec<IterationData>,
    pub summary: Summary,
    /// Backend that gyroflow-core actually dispatched to ("OpenCL", "wgpu", "" for CPU).
    /// May differ from the requested `backend` because a CPU buffer can be promoted to GPU.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual_backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IterationData {
    pub frame_us: Vec<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
pub struct Summary {
    pub mean_us: f64,
    pub median_us: f64,
    pub p95_us: f64,
    pub p99_us: f64,
    pub stddev_us: f64,
    pub min_us: u64,
    pub max_us: u64,
    pub samples: usize,
}

pub fn summarize(samples: &[u64]) -> Summary {
    if samples.is_empty() {
        return Summary { mean_us: 0.0, median_us: 0.0, p95_us: 0.0, p99_us: 0.0, stddev_us: 0.0, min_us: 0, max_us: 0, samples: 0 };
    }
    let mut sorted: Vec<u64> = samples.to_vec();
    sorted.sort_unstable();
    let n = sorted.len();
    let mean = sorted.iter().map(|x| *x as f64).sum::<f64>() / n as f64;
    let var = sorted.iter().map(|x| { let d = *x as f64 - mean; d * d }).sum::<f64>() / n as f64;
    Summary {
        mean_us: mean,
        median_us: percentile(&sorted, 50.0),
        p95_us: percentile(&sorted, 95.0),
        p99_us: percentile(&sorted, 99.0),
        stddev_us: var.sqrt(),
        min_us: *sorted.first().unwrap(),
        max_us: *sorted.last().unwrap(),
        samples: n,
    }
}

fn percentile(sorted: &[u64], pct: f64) -> f64 {
    if sorted.is_empty() { return 0.0; }
    let rank = (pct / 100.0) * (sorted.len() - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi { return sorted[lo] as f64; }
    let frac = rank - lo as f64;
    sorted[lo] as f64 * (1.0 - frac) + sorted[hi] as f64 * frac
}
