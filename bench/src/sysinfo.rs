use std::path::Path;
use std::process::Command;

use crate::result::{GitInfo, HostInfo};

pub fn collect_host() -> HostInfo {
    HostInfo {
        hostname: hostname(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        cpu_model: cpu_model(),
        cpu_cores: std::thread::available_parallelism().map(|x| x.get()).unwrap_or(0),
    }
}

pub fn collect_git(repo_root: &Path) -> GitInfo {
    GitInfo {
        plugin_rev: plugin_rev(repo_root),
        core_rev: core_rev(repo_root),
    }
}

fn hostname() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        let h = h.trim();
        if !h.is_empty() { return h.to_string(); }
    }
    if let Some(h) = Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    { return h; }
    #[cfg(target_os = "macos")]
    if let Some(h) = Command::new("scutil").args(["--get", "ComputerName"])
        .output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    { return h; }
    "unknown".to_string()
}

fn cpu_model() -> String {
    #[cfg(target_os = "macos")]
    {
        if let Ok(o) = Command::new("sysctl").args(["-n", "machdep.cpu.brand_string"]).output() {
            if let Ok(s) = String::from_utf8(o.stdout) {
                let s = s.trim();
                if !s.is_empty() { return s.to_string(); }
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(s) = std::fs::read_to_string("/proc/cpuinfo") {
            for line in s.lines() {
                if let Some(rest) = line.strip_prefix("model name") {
                    if let Some(idx) = rest.find(':') {
                        return rest[idx + 1..].trim().to_string();
                    }
                }
            }
        }
    }
    "unknown".to_string()
}

fn plugin_rev(repo_root: &Path) -> String {
    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_root)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn core_rev(repo_root: &Path) -> String {
    let path = repo_root.join("common").join("Cargo.toml");
    let Ok(content) = std::fs::read_to_string(&path) else { return "unknown".to_string(); };
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') { continue; }
        if !trimmed.contains("gyroflow-core") { continue; }
        if let Some(start) = trimmed.find("rev = \"") {
            let rest = &trimmed[start + 7..];
            if let Some(end) = rest.find('"') {
                return rest[..end].to_string();
            }
        }
    }
    "unknown".to_string()
}

pub fn repo_root() -> std::path::PathBuf {
    Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| std::path::PathBuf::from(s.trim()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}
