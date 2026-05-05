mod cli;
mod compare;
mod result;
mod runner;
mod source;
mod sysinfo;

use anyhow::{anyhow, Result};
use clap::Parser;
use std::path::PathBuf;

use crate::cli::{Cli, Cmd};
use crate::result::{BenchResult, RunConfig, SCHEMA_VERSION};

/// Default location for benchmark result files. Mirrors the standalone
/// gyroflow-core `gyroflow-bench` harness so both tools share storage and the
/// same results are not committed to the repo.
fn default_bench_dir() -> PathBuf {
    let dir = gyroflow_plugin_base::gyroflow_core::settings::data_dir().join("benchmarks");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Run(args) => cmd_run(args),
        Cmd::Compare(args) => {
            let dir = args.dir.unwrap_or_else(default_bench_dir);
            compare::compare(&args.baseline, &args.candidate, &dir, args.threshold)
        }
        Cmd::List(args) => {
            let dir = args.dir.unwrap_or_else(default_bench_dir);
            compare::list(&dir)
        }
    }
}

fn cmd_run(args: cli::RunArgs) -> Result<()> {
    let output_dir = args.output.clone().unwrap_or_else(default_bench_dir);
    std::fs::create_dir_all(&output_dir)?;
    let out_path = output_dir.join(format!("{}.bench.json", sanitize(&args.name)));
    if out_path.exists() {
        return Err(anyhow!(
            "Run '{}' already exists at {}. Choose a different --name or delete the file.",
            args.name, out_path.display()
        ));
    }

    let repo_root = sysinfo::repo_root();
    let host = sysinfo::collect_host();
    let git = sysinfo::collect_git(&repo_root);
    let frame_source = match &args.video {
        Some(v) => format!("video:{}", v.display()),
        None => "synthetic".to_string(),
    };

    println!("name:    {}", args.name);
    println!("host:    {} ({} {}, {} cores) — {}", host.hostname, host.os, host.arch, host.cpu_cores, host.cpu_model);
    println!("plugin:  {}", git.plugin_rev);
    println!("core:    {}", git.core_rev);
    println!("project: {}", args.project.display());

    let eff = runner::resolve_config(&args)?;
    let (in_w, in_h) = eff.input_size;
    let (out_w, out_h) = eff.output_size;
    let override_tag = if eff.size_override.is_some() { " (CLI override)" } else { " (from project)" };
    println!("input:   {}x{}{}", in_w, in_h, override_tag);
    println!("output:  {}x{}{}", out_w, out_h, override_tag);
    let frames_tag = if args.frames.is_some() { "(CLI)" } else { "(from project)" };
    println!("frames:  {} {} x {} iters (warmup {}, fps {:.3})", eff.frames, frames_tag, eff.iterations, eff.warmup, eff.fps);
    println!();

    let config = RunConfig {
        project: args.project.display().to_string(),
        input_size: [in_w, in_h],
        output_size: [out_w, out_h],
        frames: eff.frames,
        warmup: eff.warmup,
        iterations: eff.iterations,
        frame_source,
    };

    let cells = runner::run(&args, &eff)?;

    let now = chrono::Utc::now();
    let result = BenchResult {
        schema_version: SCHEMA_VERSION,
        timestamp: now.to_rfc3339(),
        name: args.name.clone(),
        host,
        git,
        config,
        cells,
    };

    std::fs::write(&out_path, serde_json::to_string_pretty(&result)?)?;
    println!("\nwrote {}", out_path.display());
    Ok(())
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}
