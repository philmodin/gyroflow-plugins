use anyhow::{anyhow, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::result::{BenchResult, Cell};

pub fn load(path: &Path) -> Result<BenchResult> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("failed to parse {}", path.display()))
}

/// Resolve a CLI argument to a result file path. The argument may be:
///   - an absolute or relative path that exists on disk;
///   - a label or filename substring that uniquely identifies a file in `dir`;
///   - a label that matches multiple files — picks the newest by mtime.
pub fn resolve(arg: &str, dir: &Path) -> Result<PathBuf> {
    let direct = PathBuf::from(arg);
    if direct.exists() { return Ok(direct); }

    if !dir.exists() {
        return Err(anyhow!("'{}' is not a path and {} does not exist", arg, dir.display()));
    }
    let json_files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
        .collect();
    // Prefer files whose stored label equals `arg` exactly; fall back to filename substring.
    let exact: Vec<PathBuf> = json_files.iter()
        .filter(|p| load(p).ok().map(|r| r.label == arg).unwrap_or(false))
        .cloned().collect();
    let mut matches = if !exact.is_empty() { exact } else {
        json_files.iter()
            .filter(|p| p.file_name().and_then(|n| n.to_str()).map(|n| n.contains(arg)).unwrap_or(false))
            .cloned().collect::<Vec<_>>()
    };
    if matches.is_empty() {
        return Err(anyhow!("no result file in {} matches '{}'", dir.display(), arg));
    }
    if matches.len() > 1 {
        matches.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
        let chosen = matches.last().unwrap().clone();
        eprintln!("note: '{}' matched {} files; using newest: {}",
            arg, matches.len(), chosen.file_name().unwrap().to_string_lossy());
        return Ok(chosen);
    }
    Ok(matches.pop().unwrap())
}

pub fn compare(baseline: &str, candidate: &str, dir: &Path, threshold_pct: f64) -> Result<()> {
    let baseline_path = resolve(baseline, dir)?;
    let candidate_path = resolve(candidate, dir)?;
    let base = load(&baseline_path)?;
    let cand = load(&candidate_path)?;

    println!("baseline:  {}  (plugin {}, core {})", baseline_path.display(), short(&base.git.plugin_rev), short(&base.git.core_rev));
    println!("candidate: {}  (plugin {}, core {})", candidate_path.display(), short(&cand.git.plugin_rev), short(&cand.git.core_rev));
    println!();

    let base_cells = index_cells(&base.cells);
    let cand_cells = index_cells(&cand.cells);

    println!("{:<8} {:<8} {:>12} {:>12} {:>10} {:>20} {}",
        "backend", "format", "base_med_us", "cand_med_us", "delta_%", "ci95_pct", "verdict");
    println!("{}", "-".repeat(90));

    let mut keys: Vec<&(String, String)> = base_cells.keys().collect();
    keys.extend(cand_cells.keys().filter(|k| !base_cells.contains_key(*k)));
    keys.sort();

    for key in keys {
        let b = base_cells.get(key);
        let c = cand_cells.get(key);
        match (b, c) {
            (Some(b), Some(c)) => {
                if b.note.is_some() || c.note.is_some() || b.summary.samples == 0 || c.summary.samples == 0 {
                    println!("{:<8} {:<8} {:>12} {:>12} {:>10} {:>20} {}",
                        key.0, key.1, "-", "-", "-", "-",
                        b.note.as_deref().or(c.note.as_deref()).unwrap_or("(no samples)"));
                    continue;
                }
                let bm = b.summary.median_us;
                let cm = c.summary.median_us;
                let delta = (cm - bm) / bm * 100.0;
                let (lo, hi) = bootstrap_ci_pct(b, c, 1000);
                let verdict = if delta > threshold_pct && lo > 0.0 {
                    "REGRESSION"
                } else if delta < -threshold_pct && hi < 0.0 {
                    "WIN"
                } else {
                    "noise"
                };
                println!("{:<8} {:<8} {:>12.1} {:>12.1} {:>+9.2}% {:>+9.2}..{:>+6.2}% {}",
                    key.0, key.1, bm, cm, delta, lo, hi, verdict);
            }
            (Some(_), None) => println!("{:<8} {:<8} {:>12} {:>12} {:>10} {:>20} only in baseline", key.0, key.1, "-", "-", "-", "-"),
            (None, Some(_)) => println!("{:<8} {:<8} {:>12} {:>12} {:>10} {:>20} only in candidate", key.0, key.1, "-", "-", "-", "-"),
            (None, None) => {}
        }
    }
    Ok(())
}

fn index_cells(cells: &[Cell]) -> BTreeMap<(String, String), &Cell> {
    cells.iter().map(|c| ((c.backend.clone(), c.pixel_format.clone()), c)).collect()
}

fn short(s: &str) -> &str { if s.len() > 7 { &s[..7] } else { s } }

/// Percentile bootstrap CI for the percent change of medians.
fn bootstrap_ci_pct(base: &Cell, cand: &Cell, iters: usize) -> (f64, f64) {
    let b: Vec<u64> = base.iterations.iter().flat_map(|i| i.frame_us.iter().copied()).collect();
    let c: Vec<u64> = cand.iterations.iter().flat_map(|i| i.frame_us.iter().copied()).collect();
    if b.is_empty() || c.is_empty() { return (0.0, 0.0); }
    let mut rng = SimpleRng::new(0xC0FFEE);
    let mut deltas = Vec::with_capacity(iters);
    let mut buf_b = vec![0u64; b.len()];
    let mut buf_c = vec![0u64; c.len()];
    for _ in 0..iters {
        for slot in buf_b.iter_mut() { *slot = b[rng.next_usize(b.len())]; }
        for slot in buf_c.iter_mut() { *slot = c[rng.next_usize(c.len())]; }
        let bm = median(&mut buf_b);
        let cm = median(&mut buf_c);
        if bm > 0.0 {
            deltas.push((cm - bm) / bm * 100.0);
        }
    }
    deltas.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if deltas.is_empty() { return (0.0, 0.0); }
    let lo_idx = ((deltas.len() as f64) * 0.025) as usize;
    let hi_idx = (((deltas.len() as f64) * 0.975) as usize).min(deltas.len() - 1);
    (deltas[lo_idx], deltas[hi_idx])
}

fn median(v: &mut [u64]) -> f64 {
    v.sort_unstable();
    let n = v.len();
    if n == 0 { return 0.0; }
    if n % 2 == 1 { v[n / 2] as f64 } else { (v[n / 2 - 1] as f64 + v[n / 2] as f64) / 2.0 }
}

struct SimpleRng { state: u64 }
impl SimpleRng {
    fn new(seed: u64) -> Self { Self { state: seed | 1 } }
    fn next_u64(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.state;
        x ^= x >> 12; x ^= x << 25; x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn next_usize(&mut self, bound: usize) -> usize { (self.next_u64() as usize) % bound }
}

pub fn list(dir: &Path) -> Result<()> {
    if !dir.exists() {
        println!("(no results dir at {})", dir.display());
        return Ok(());
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        match load(&path) {
            Ok(r) => println!("{}  label={:<20} plugin={} core={}",
                path.file_name().unwrap().to_string_lossy(),
                r.label,
                short(&r.git.plugin_rev),
                short(&r.git.core_rev)),
            Err(err) => println!("{}  (parse error: {err})", path.display()),
        }
    }
    Ok(())
}
