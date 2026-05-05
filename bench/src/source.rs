use anyhow::{anyhow, Context, Result};
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};

pub enum FrameSource {
    Synthetic { template: Vec<u8> },
    Video { child: Child, frame_bytes: usize },
}

impl FrameSource {
    pub fn synthetic(frame_bytes: usize) -> Self {
        let mut template = vec![0u8; frame_bytes];
        for (i, b) in template.iter_mut().enumerate() {
            *b = (i & 0xFF) as u8;
        }
        FrameSource::Synthetic { template }
    }

    /// Spawn `ffmpeg -i <video> -f rawvideo -pix_fmt rgba -` and stream raw rgba frames.
    pub fn video(video: &Path, width: usize, height: usize) -> Result<Self> {
        let frame_bytes = width * height * 4;
        let child = Command::new("ffmpeg")
            .args(["-loglevel", "error", "-i"])
            .arg(video)
            .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{}x{}", width, height), "-"])
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("failed to spawn ffmpeg for {}", video.display()))?;
        Ok(FrameSource::Video { child, frame_bytes })
    }

    /// Fill `dst` with the next frame. For synthetic, copies the template (cheap).
    /// For video, reads from the ffmpeg pipe; on EOF, returns Err.
    pub fn fill(&mut self, dst: &mut [u8]) -> Result<()> {
        match self {
            FrameSource::Synthetic { template } => {
                let n = dst.len().min(template.len());
                dst[..n].copy_from_slice(&template[..n]);
                Ok(())
            }
            FrameSource::Video { child, frame_bytes } => {
                if dst.len() < *frame_bytes {
                    return Err(anyhow!("dst buffer smaller than frame_bytes"));
                }
                let stdout = child.stdout.as_mut().ok_or_else(|| anyhow!("ffmpeg stdout missing"))?;
                stdout.read_exact(&mut dst[..*frame_bytes]).context("ffmpeg pipe read failed (EOF?)")?;
                Ok(())
            }
        }
    }
}

impl Drop for FrameSource {
    fn drop(&mut self) {
        if let FrameSource::Video { child, .. } = self {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
