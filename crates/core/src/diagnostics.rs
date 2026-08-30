//! Bounded path-free structured application logs.

use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use thiserror::Error;

const LOG_SCHEMA_VERSION: u16 = 1;
const DEFAULT_MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;
const MAX_RECORD_BYTES: usize = 4 * 1024;
const MAX_PRODUCT_BYTES: usize = 32;
const MAX_EVENT_BYTES: usize = 64;
const MAX_CODE_BYTES: usize = 128;
const RETAINED_LOG_FILES: usize = 16;

static GLOBAL_LOG: OnceLock<StructuredLog> = OnceLock::new();

/// Stable structured-log severity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogLevel {
    /// Normal lifecycle evidence.
    Info,
    /// Recoverable degraded state.
    Warn,
    /// A command or runtime operation failed.
    Error,
}

impl LogLevel {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// Structured-log initialization or write failure.
#[derive(Debug, Error)]
pub enum DiagnosticLogError {
    /// Product, event, or code was not a bounded path-free token.
    #[error("diagnostic log field is not a bounded token")]
    InvalidField,
    /// The configured file budget is unusable.
    #[error("diagnostic log byte budget is invalid")]
    InvalidBudget,
    /// A serialized record exceeded its fixed bound.
    #[error("diagnostic log record exceeded its byte bound")]
    RecordTooLarge,
    /// The process-global writer was initialized already.
    #[error("diagnostic log is already initialized")]
    AlreadyInitialized,
    /// The writer mutex was poisoned.
    #[error("diagnostic log writer is unavailable")]
    WriterUnavailable,
    /// Local log-directory or file I/O failed.
    #[error("diagnostic log I/O failed")]
    Io(#[from] std::io::Error),
    /// The closed record could not be encoded.
    #[error("diagnostic log record encoding failed")]
    Encode(#[from] serde_json::Error),
}

#[derive(Serialize)]
struct LogRecord<'a> {
    schema_version: u16,
    timestamp_unix_ms: u64,
    level: &'static str,
    event: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'a str>,
}

struct LogState {
    file: File,
    written: u64,
    max_bytes: u64,
}

/// Cloneable single-process JSONL writer with whole-record bounded writes.
#[derive(Clone)]
pub struct StructuredLog {
    state: Arc<Mutex<LogState>>,
    file_name: Arc<str>,
}

impl StructuredLog {
    /// Open a normal application log and retain at most sixteen matching files.
    ///
    /// # Errors
    ///
    /// Returns a validation or local I/O error without exposing a path in a
    /// structured record.
    pub fn open(directory: &Path, product: &str) -> Result<Self, DiagnosticLogError> {
        validate_token(product, MAX_PRODUCT_BYTES)?;
        fs::create_dir_all(directory)?;
        prune_product_logs(directory, product, RETAINED_LOG_FILES.saturating_sub(1));
        Self::open_with_limit(directory, product, DEFAULT_MAX_LOG_BYTES)
    }

    /// Open a writer with an explicit byte budget. Intended for bounded tests
    /// and diagnostic hosts with a stricter local policy.
    ///
    /// # Errors
    ///
    /// Returns a validation or local I/O error.
    pub fn open_with_limit(
        directory: &Path,
        product: &str,
        max_bytes: u64,
    ) -> Result<Self, DiagnosticLogError> {
        validate_token(product, MAX_PRODUCT_BYTES)?;
        if !(256..=64 * 1024 * 1024).contains(&max_bytes) {
            return Err(DiagnosticLogError::InvalidBudget);
        }
        fs::create_dir_all(directory)?;
        let timestamp = unix_nanos()?;
        let process = std::process::id();
        let (file, file_name) = (0_u8..8)
            .find_map(|attempt| {
                let suffix = if attempt == 0 {
                    String::new()
                } else {
                    format!("-{attempt}")
                };
                let file_name = format!("{product}-{process}-{timestamp}{suffix}.jsonl");
                let path = directory.join(&file_name);
                match OpenOptions::new().write(true).create_new(true).open(path) {
                    Ok(file) => Some(Ok((file, file_name))),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                    Err(error) => Some(Err(error)),
                }
            })
            .transpose()?
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::AlreadyExists, "log collision")
            })?;
        Ok(Self {
            state: Arc::new(Mutex::new(LogState {
                file,
                written: 0,
                max_bytes,
            })),
            file_name: Arc::from(file_name),
        })
    }

    /// Basename only; the machine-local log directory is never exposed.
    #[must_use]
    pub fn file_name(&self) -> &str {
        &self.file_name
    }

    /// Append one complete JSON record or drop it when the file budget is full.
    ///
    /// # Errors
    ///
    /// Rejects non-token fields, poisoned state, encoding failure, or local I/O.
    pub fn record(
        &self,
        level: LogLevel,
        event: &str,
        code: Option<&str>,
    ) -> Result<(), DiagnosticLogError> {
        validate_token(event, MAX_EVENT_BYTES)?;
        if let Some(code) = code {
            validate_token(code, MAX_CODE_BYTES)?;
        }
        let record = LogRecord {
            schema_version: LOG_SCHEMA_VERSION,
            timestamp_unix_ms: unix_millis()?,
            level: level.as_str(),
            event,
            code,
        };
        let mut encoded = serde_json::to_vec(&record)?;
        encoded.push(b'\n');
        if encoded.len() > MAX_RECORD_BYTES {
            return Err(DiagnosticLogError::RecordTooLarge);
        }
        let encoded_len =
            u64::try_from(encoded.len()).map_err(|_| DiagnosticLogError::RecordTooLarge)?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| DiagnosticLogError::WriterUnavailable)?;
        let Some(next_size) = state.written.checked_add(encoded_len) else {
            return Ok(());
        };
        if next_size > state.max_bytes {
            return Ok(());
        }
        state.file.write_all(&encoded)?;
        state.file.flush()?;
        state.written = next_size;
        Ok(())
    }
}

/// Install the process-global bounded writer used by application boundaries.
///
/// # Errors
///
/// Returns initialization, validation, or local I/O failures.
pub fn initialize_global_json_log(
    directory: &Path,
    product: &str,
) -> Result<&'static str, DiagnosticLogError> {
    let log = StructuredLog::open(directory, product)?;
    GLOBAL_LOG
        .set(log)
        .map_err(|_| DiagnosticLogError::AlreadyInitialized)?;
    GLOBAL_LOG
        .get()
        .map(StructuredLog::file_name)
        .ok_or(DiagnosticLogError::WriterUnavailable)
}

/// Best-effort event recording after process initialization.
pub fn record_global(level: LogLevel, event: &str, code: Option<&str>) {
    if let Some(log) = GLOBAL_LOG.get() {
        let _ = log.record(level, event, code);
    }
}

fn validate_token(value: &str, max_bytes: usize) -> Result<(), DiagnosticLogError> {
    if value.is_empty()
        || value.len() > max_bytes
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(DiagnosticLogError::InvalidField);
    }
    Ok(())
}

fn unix_millis() -> Result<u64, DiagnosticLogError> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| std::io::Error::other("system clock precedes Unix epoch"))?;
    u64::try_from(elapsed.as_millis()).map_err(|_| DiagnosticLogError::InvalidBudget)
}

fn unix_nanos() -> Result<u128, DiagnosticLogError> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| std::io::Error::other("system clock precedes Unix epoch"))?
        .as_nanos())
}

fn prune_product_logs(directory: &Path, product: &str, keep: usize) {
    let prefix = format!("{product}-");
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut candidates = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            if !file_type.is_file() {
                return None;
            }
            let name = entry.file_name();
            let rendered = name.to_str()?;
            if !rendered.starts_with(&prefix)
                || !Path::new(rendered)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"))
            {
                return None;
            }
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, rendered.to_owned(), entry.path()))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    let remove_count = candidates.len().saturating_sub(keep);
    for (_, _, path) in candidates.into_iter().take(remove_count) {
        let _ = fs::remove_file(path);
    }
}
