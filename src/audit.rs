use crate::{
    error::{HarnessError, Result},
    models::Point,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Mutex,
};
use uuid::Uuid;

const MAX_BYTES: u64 = 10 * 1024 * 1024;
const ARCHIVES: usize = 5;

#[derive(Debug, Serialize)]
pub struct AuditRecord {
    pub timestamp: DateTime<Utc>,
    pub session_id: Uuid,
    pub request_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_index: Option<usize>,
    pub tool: String,
    pub arguments: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_window: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_before: Option<Point>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_after: Option<Point>,
    pub duration_ms: u128,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Debug)]
pub struct AuditLogger {
    path: PathBuf,
    lock: Mutex<()>,
}

impl AuditLogger {
    pub fn new(path: Option<PathBuf>) -> Result<Self> {
        let path = path.unwrap_or_else(default_audit_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| HarnessError::io("AUDIT_UNAVAILABLE", "create audit directory", e))?;
        }
        let logger = Self {
            path,
            lock: Mutex::new(()),
        };
        logger.ensure_writable()?;
        Ok(logger)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn ensure_writable(&self) -> Result<()> {
        let _guard = self.lock.lock().map_err(|_| {
            HarnessError::new("AUDIT_UNAVAILABLE", "audit logger state is unavailable")
        })?;
        OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&self.path)
            .map_err(|e| HarnessError::io("AUDIT_UNAVAILABLE", "open audit log", e))?;
        fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))
            .map_err(|e| HarnessError::io("AUDIT_UNAVAILABLE", "secure audit log", e))
    }

    pub fn record(&self, record: &AuditRecord) -> Result<()> {
        let _guard = self.lock.lock().map_err(|_| {
            HarnessError::new("AUDIT_UNAVAILABLE", "audit logger state is unavailable")
        })?;
        self.rotate_if_needed()?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&self.path)
            .map_err(|e| HarnessError::io("AUDIT_UNAVAILABLE", "open audit log", e))?;
        serde_json::to_writer(&mut file, record)
            .map_err(|e| HarnessError::io("AUDIT_UNAVAILABLE", "serialize audit record", e))?;
        file.write_all(b"\n")
            .and_then(|_| file.flush())
            .map_err(|e| HarnessError::io("AUDIT_UNAVAILABLE", "write audit record", e))
    }

    fn rotate_if_needed(&self) -> Result<()> {
        if self.path.metadata().map(|m| m.len()).unwrap_or(0) < MAX_BYTES {
            return Ok(());
        }
        let archive = |index: usize| PathBuf::from(format!("{}.{}", self.path.display(), index));
        let _ = fs::remove_file(archive(ARCHIVES));
        for index in (1..ARCHIVES).rev() {
            let old = archive(index);
            if old.exists() {
                fs::rename(&old, archive(index + 1)).map_err(|e| {
                    HarnessError::io("AUDIT_UNAVAILABLE", "rotate audit archive", e)
                })?;
            }
        }
        fs::rename(&self.path, archive(1))
            .map_err(|e| HarnessError::io("AUDIT_UNAVAILABLE", "rotate audit log", e))
    }
}

pub fn default_audit_path() -> PathBuf {
    if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
        PathBuf::from(state).join("hyprharness/audit.jsonl")
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".local/state/hyprharness/audit.jsonl")
    } else {
        PathBuf::from("/tmp/hyprharness-audit.jsonl")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn writes_jsonl_record() {
        let dir = tempdir().unwrap();
        let logger = AuditLogger::new(Some(dir.path().join("audit.jsonl"))).unwrap();
        let record = AuditRecord {
            timestamp: Utc::now(),
            session_id: Uuid::nil(),
            request_id: Uuid::nil(),
            sequence_id: None,
            step_index: None,
            tool: "get_cursor".into(),
            arguments: serde_json::json!({}),
            active_window: None,
            cursor_before: None,
            cursor_after: None,
            duration_ms: 1,
            success: true,
            error_code: None,
        };
        logger.record(&record).unwrap();
        let text = fs::read_to_string(logger.path()).unwrap();
        let value: Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(value["tool"], "get_cursor");
    }

    #[test]
    fn rotates_at_size_limit() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let logger = AuditLogger::new(Some(path.clone())).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_BYTES)
            .unwrap();
        let record = AuditRecord {
            timestamp: Utc::now(),
            session_id: Uuid::nil(),
            request_id: Uuid::nil(),
            sequence_id: None,
            step_index: None,
            tool: "rotation_test".into(),
            arguments: serde_json::json!({}),
            active_window: None,
            cursor_before: None,
            cursor_after: None,
            duration_ms: 0,
            success: true,
            error_code: None,
        };
        logger.record(&record).unwrap();
        assert!(PathBuf::from(format!("{}.1", path.display())).exists());
        assert!(path.metadata().unwrap().len() < MAX_BYTES);
    }
}
