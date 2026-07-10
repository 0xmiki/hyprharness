use crate::{
    error::{HarnessError, Result},
    models::{CursorPosition, LockStatus, Monitor, Point, Window},
};
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use std::{env, path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    time::timeout,
};

const IPC_TIMEOUT: Duration = Duration::from_secs(3);

#[async_trait]
pub trait HyprlandApi: Send + Sync {
    async fn monitors(&self) -> Result<Vec<Monitor>>;
    async fn windows(&self) -> Result<Vec<Window>>;
    async fn cursor(&self) -> Result<Point>;
    async fn locked(&self) -> Result<bool>;
    async fn move_cursor(&self, point: Point) -> Result<()>;
    async fn version(&self) -> Result<serde_json::Value>;
    async fn get_option(&self, name: &str) -> Result<serde_json::Value>;
}

#[derive(Debug, Clone)]
pub struct HyprlandIpc {
    socket_path: PathBuf,
}

impl HyprlandIpc {
    pub fn from_env() -> Result<Self> {
        let runtime = env::var_os("XDG_RUNTIME_DIR")
            .ok_or_else(|| HarnessError::new("NOT_HYPRLAND", "XDG_RUNTIME_DIR is not set"))?;
        let signature = env::var_os("HYPRLAND_INSTANCE_SIGNATURE").ok_or_else(|| {
            HarnessError::new("NOT_HYPRLAND", "HYPRLAND_INSTANCE_SIGNATURE is not set")
        })?;
        let socket_path = PathBuf::from(runtime)
            .join("hypr")
            .join(signature)
            .join(".socket.sock");
        if !socket_path.exists() {
            return Err(HarnessError::new(
                "IPC_UNAVAILABLE",
                format!(
                    "Hyprland IPC socket does not exist: {}",
                    socket_path.display()
                ),
            ));
        }
        Ok(Self { socket_path })
    }

    pub fn socket_path(&self) -> &std::path::Path {
        &self.socket_path
    }

    async fn request_bytes(&self, request: String) -> Result<Vec<u8>> {
        let path = self.socket_path.clone();
        timeout(IPC_TIMEOUT, async move {
            let mut stream = UnixStream::connect(&path)
                .await
                .map_err(|e| HarnessError::io("IPC_UNAVAILABLE", "connect Hyprland IPC", e))?;
            stream
                .write_all(request.as_bytes())
                .await
                .map_err(|e| HarnessError::io("IPC_UNAVAILABLE", "write Hyprland IPC", e))?;
            stream.shutdown().await.map_err(|e| {
                HarnessError::io("IPC_UNAVAILABLE", "close Hyprland IPC request", e)
            })?;
            let mut response = Vec::new();
            stream
                .read_to_end(&mut response)
                .await
                .map_err(|e| HarnessError::io("IPC_UNAVAILABLE", "read Hyprland IPC", e))?;
            Ok(response)
        })
        .await
        .map_err(|_| HarnessError::new("IPC_UNAVAILABLE", "Hyprland IPC timed out"))?
    }

    async fn json<T: DeserializeOwned>(&self, command: &str) -> Result<T> {
        let bytes = self.request_bytes(format!("j/{command}")).await?;
        serde_json::from_slice(&bytes)
            .map_err(|e| HarnessError::io("IPC_UNAVAILABLE", "parse Hyprland JSON response", e))
    }

    async fn dispatch(&self, command: &str) -> Result<()> {
        let bytes = self.request_bytes(format!("dispatch {command}")).await?;
        let response = String::from_utf8_lossy(&bytes);
        if response.trim() == "ok" {
            Ok(())
        } else {
            Err(HarnessError::new(
                "IPC_UNAVAILABLE",
                format!("Hyprland dispatcher rejected command: {}", response.trim()),
            ))
        }
    }
}

#[async_trait]
impl HyprlandApi for HyprlandIpc {
    async fn monitors(&self) -> Result<Vec<Monitor>> {
        self.json("monitors").await
    }

    async fn windows(&self) -> Result<Vec<Window>> {
        let mut windows: Vec<Window> = self.json("clients").await?;
        let active: Option<Window> = self.json("activewindow").await.ok();
        if let Some(active) = active {
            for window in &mut windows {
                window.focused = window.address == active.address;
            }
        } else if let Some(window) = windows
            .iter_mut()
            .find(|window| window.focus_history_id == 0)
        {
            window.focused = true;
        }
        Ok(windows)
    }

    async fn cursor(&self) -> Result<Point> {
        let cursor: CursorPosition = self.json("cursorpos").await?;
        Ok(cursor.into())
    }

    async fn locked(&self) -> Result<bool> {
        let status: LockStatus = self.json("locked").await?;
        Ok(status.locked)
    }

    async fn move_cursor(&self, point: Point) -> Result<()> {
        self.dispatch(&format!("movecursor {} {}", point.x, point.y))
            .await
    }

    async fn version(&self) -> Result<serde_json::Value> {
        self.json("version").await
    }

    async fn get_option(&self, name: &str) -> Result<serde_json::Value> {
        self.json(&format!("getoption {name}")).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use tokio::net::UnixListener;

    #[tokio::test]
    async fn sends_request_and_parses_json() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("socket");
        let listener = UnixListener::bind(&path).unwrap();
        let ipc = HyprlandIpc { socket_path: path };
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            stream.read_to_end(&mut request).await.unwrap();
            assert_eq!(request, b"j/cursorpos");
            stream.write_all(br#"{"x":10,"y":20}"#).await.unwrap();
        });
        assert_eq!(ipc.cursor().await.unwrap(), Point { x: 10, y: 20 });
        server.await.unwrap();
    }
}
