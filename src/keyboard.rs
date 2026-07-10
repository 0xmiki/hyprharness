use crate::{
    error::{HarnessError, Result},
    models::KeyModifier,
};
use async_trait::async_trait;
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{io::AsyncWriteExt, process::Command, time::timeout};

const KEYBOARD_TIMEOUT: Duration = Duration::from_secs(35);

#[async_trait]
pub trait KeyboardApi: Send + Sync {
    async fn press_key(&self, key: &str, modifiers: &[KeyModifier], repeat: u8) -> Result<()>;
    async fn type_text(&self, text: &str, interval_ms: u32) -> Result<()>;
    async fn probe(&self) -> Result<()>;
    fn executable(&self) -> &std::path::Path;
}

#[derive(Debug, Clone)]
pub struct WtypeKeyboard {
    executable: PathBuf,
}

impl WtypeKeyboard {
    pub fn discover() -> Result<Self> {
        let executable = std::env::var_os("PATH")
            .and_then(|paths| {
                std::env::split_paths(&paths)
                    .map(|path| path.join("wtype"))
                    .find(|path| path.is_file())
            })
            .ok_or_else(|| {
                HarnessError::new("KEYBOARD_UNAVAILABLE", "wtype was not found in PATH")
            })?;
        Ok(Self { executable })
    }

    pub fn from_path(executable: PathBuf) -> Self {
        Self { executable }
    }

    async fn run(&self, args: &[String], stdin: Option<&[u8]>) -> Result<()> {
        let mut child = Command::new(&self.executable)
            .args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| HarnessError::io("KEYBOARD_UNAVAILABLE", "start wtype", error))?;
        if let Some(data) = stdin {
            let mut writer = child.stdin.take().ok_or_else(|| {
                HarnessError::new("KEYBOARD_UNAVAILABLE", "wtype stdin was not available")
            })?;
            writer.write_all(data).await.map_err(|error| {
                HarnessError::io("KEYBOARD_UNAVAILABLE", "write wtype stdin", error)
            })?;
        }
        let output = timeout(KEYBOARD_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| HarnessError::new("KEYBOARD_UNAVAILABLE", "wtype timed out"))?
            .map_err(|error| HarnessError::io("KEYBOARD_UNAVAILABLE", "wait for wtype", error))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(HarnessError::new(
                "KEYBOARD_UNAVAILABLE",
                format!(
                    "wtype exited with {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ))
        }
    }
}

#[async_trait]
impl KeyboardApi for WtypeKeyboard {
    async fn press_key(&self, key: &str, modifiers: &[KeyModifier], repeat: u8) -> Result<()> {
        let keysym = normalize_key(key)?;
        let mut args = Vec::with_capacity(modifiers.len() * 4 + repeat as usize * 2);
        for modifier in modifiers {
            args.extend(["-M".into(), modifier.wtype_name().into()]);
        }
        for _ in 0..repeat {
            args.extend(["-k".into(), keysym.clone()]);
        }
        for modifier in modifiers.iter().rev() {
            args.extend(["-m".into(), modifier.wtype_name().into()]);
        }
        self.run(&args, None).await
    }

    async fn type_text(&self, text: &str, interval_ms: u32) -> Result<()> {
        if text.contains('\0') {
            return Err(HarnessError::invalid("text cannot contain a NUL character"));
        }
        self.run(
            &["-d".into(), interval_ms.to_string(), "-".into()],
            Some(text.as_bytes()),
        )
        .await
    }

    async fn probe(&self) -> Result<()> {
        self.run(&["--".into(), String::new()], None).await
    }

    fn executable(&self) -> &std::path::Path {
        &self.executable
    }
}

pub fn normalize_key(key: &str) -> Result<String> {
    if key.len() == 1 && key.as_bytes()[0].is_ascii_alphanumeric() {
        return Ok(key.to_ascii_lowercase());
    }
    let keysym = match key.to_ascii_lowercase().as_str() {
        "enter" | "return" => "Return",
        "escape" | "esc" => "Escape",
        "tab" => "Tab",
        "space" => "space",
        "backspace" => "BackSpace",
        "delete" => "Delete",
        "insert" => "Insert",
        "left" => "Left",
        "right" => "Right",
        "up" => "Up",
        "down" => "Down",
        "home" => "Home",
        "end" => "End",
        "page_up" | "pageup" => "Page_Up",
        "page_down" | "pagedown" => "Page_Down",
        "minus" => "minus",
        "equal" => "equal",
        "comma" => "comma",
        "period" | "dot" => "period",
        "slash" => "slash",
        "semicolon" => "semicolon",
        "apostrophe" => "apostrophe",
        "bracket_left" => "bracketleft",
        "bracket_right" => "bracketright",
        "backslash" => "backslash",
        "grave" => "grave",
        value
            if value.len() >= 2
                && value.len() <= 3
                && value.starts_with('f')
                && value[1..]
                    .parse::<u8>()
                    .is_ok_and(|number| (1..=12).contains(&number)) =>
        {
            return Ok(value.to_ascii_uppercase());
        }
        _ => {
            return Err(HarnessError::invalid(format!(
                "unsupported key '{key}'; use a letter, digit, F1-F12, or a documented named key"
            )));
        }
    };
    Ok(keysym.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};
    use tempfile::tempdir;

    #[test]
    fn normalizes_supported_keys() {
        assert_eq!(normalize_key("enter").unwrap(), "Return");
        assert_eq!(normalize_key("page_up").unwrap(), "Page_Up");
        assert_eq!(normalize_key("F12").unwrap(), "F12");
        assert_eq!(normalize_key("L").unwrap(), "l");
    }

    #[test]
    fn rejects_unknown_keys() {
        assert_eq!(
            normalize_key("volume_up").unwrap_err().code,
            "INVALID_ARGUMENT"
        );
    }

    #[tokio::test]
    async fn sends_typed_text_over_stdin_not_process_arguments() {
        let dir = tempdir().unwrap();
        let script = dir.path().join("fake-wtype");
        let args_file = dir.path().join("args");
        let stdin_file = dir.path().join("stdin");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\ncat > '{}'\n",
                args_file.display(),
                stdin_file.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let keyboard = WtypeKeyboard::from_path(script);
        keyboard.type_text("private text", 7).await.unwrap();
        let args = fs::read_to_string(args_file).unwrap();
        assert_eq!(args, "-d\n7\n-\n");
        assert!(!args.contains("private text"));
        assert_eq!(fs::read_to_string(stdin_file).unwrap(), "private text");
    }
}
