use crate::error::{HarnessError, Result};
use async_trait::async_trait;
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command, time::timeout};

#[derive(Debug, Clone)]
pub struct Capture {
    pub png: Vec<u8>,
    pub pixel_width: u32,
    pub pixel_height: u32,
}

#[async_trait]
pub trait ScreenshotApi: Send + Sync {
    async fn capture_monitor(&self, monitor: &str) -> Result<Capture>;
    fn executable(&self) -> &std::path::Path;
}

#[derive(Debug, Clone)]
pub struct GrimCapture {
    executable: PathBuf,
}

impl GrimCapture {
    pub fn discover() -> Result<Self> {
        let path = std::env::var_os("PATH")
            .and_then(|paths| {
                std::env::split_paths(&paths)
                    .map(|path| path.join("grim"))
                    .find(|path| path.is_file())
            })
            .ok_or_else(|| HarnessError::new("CAPTURE_FAILED", "grim was not found in PATH"))?;
        Ok(Self { executable: path })
    }

    pub fn from_path(executable: PathBuf) -> Self {
        Self { executable }
    }
}

#[async_trait]
impl ScreenshotApi for GrimCapture {
    async fn capture_monitor(&self, monitor: &str) -> Result<Capture> {
        if monitor.is_empty() || monitor.contains('\0') {
            return Err(HarnessError::invalid("monitor name is invalid"));
        }
        let child = Command::new(&self.executable)
            .args(["-c", "-t", "png", "-o", monitor, "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| HarnessError::io("CAPTURE_FAILED", "start grim", e))?;

        let output = timeout(Duration::from_secs(10), child.wait_with_output())
            .await
            .map_err(|_| HarnessError::new("CAPTURE_FAILED", "grim timed out"))?
            .map_err(|e| HarnessError::io("CAPTURE_FAILED", "wait for grim", e))?;
        if !output.status.success() {
            return Err(HarnessError::new(
                "CAPTURE_FAILED",
                format!(
                    "grim exited with {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }
        let (pixel_width, pixel_height) = png_dimensions(&output.stdout)?;
        Ok(Capture {
            png: output.stdout,
            pixel_width,
            pixel_height,
        })
    }

    fn executable(&self) -> &std::path::Path {
        &self.executable
    }
}

pub fn png_dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return Err(HarnessError::new(
            "CAPTURE_FAILED",
            "grim returned invalid PNG data",
        ));
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    if width == 0 || height == 0 {
        return Err(HarnessError::new(
            "CAPTURE_FAILED",
            "grim returned a zero-sized image",
        ));
    }
    Ok((width, height))
}

pub async fn write_png(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| HarnessError::io("CAPTURE_FAILED", "create output directory", e))?;
    }
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|e| HarnessError::io("CAPTURE_FAILED", "create screenshot output", e))?;
    file.write_all(bytes)
        .await
        .map_err(|e| HarnessError::io("CAPTURE_FAILED", "write screenshot output", e))?;
    file.flush()
        .await
        .map_err(|e| HarnessError::io("CAPTURE_FAILED", "flush screenshot output", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_png_dimensions() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&1920u32.to_be_bytes());
        png.extend_from_slice(&1080u32.to_be_bytes());
        assert_eq!(png_dimensions(&png).unwrap(), (1920, 1080));
    }

    #[test]
    fn rejects_non_png() {
        assert_eq!(png_dimensions(b"nope").unwrap_err().code, "CAPTURE_FAILED");
    }
}
