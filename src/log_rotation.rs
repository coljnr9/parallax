use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Configuration for log file retention and cleanup.
///
/// Note: `tracing_appender::rolling::*` handles *rotation* (e.g. daily). This
/// struct governs *retention* (age/size/file-count) of the rotated artifacts.
#[derive(Clone, Debug)]
pub struct LogRotationConfig {
    /// Maximum number of rotated log files to keep as a last-resort safety valve.
    ///
    /// If both age and total-size limits are configured, those are enforced first.
    pub max_files: usize,

    /// Maximum age (in hours) of log files to retain.
    ///
    /// Default is 60 hours (~2.5 days) to comfortably cover “1–2 days” across
    /// local timezone boundaries.
    pub max_age_hours: u64,

    /// Maximum combined size (in bytes) of all matching log files.
    ///
    /// When exceeded, the oldest files are deleted until under the limit.
    pub max_total_size_bytes: u64,
}

impl Default for LogRotationConfig {
    fn default() -> Self {
        // Defaults chosen to support ~1–2 days of debugging history while still
        // preventing unbounded growth.
        Self {
            max_files: 512,
            max_age_hours: 60,
            max_total_size_bytes: 5 * 1024 * 1024 * 1024, // 5GB
        }
    }
}

/// Manages log retention cleanup for rotated log files.
#[derive(Clone, Debug)]
pub struct LogRotationManager {
    config: LogRotationConfig,
}

impl LogRotationManager {
    pub fn new(config: LogRotationConfig) -> Self {
        Self { config }
    }

    /// Applies retention policy for all log files whose file name starts with `log_prefix`.
    ///
    /// This is safe to call periodically.
    pub fn check_and_rotate(&self, log_dir: &Path, log_prefix: &str) -> std::io::Result<()> {
        let mut log_files = self.find_log_files(log_dir, log_prefix)?;
        if log_files.is_empty() {
            return Ok(());
        }

        self.cleanup_by_age(&mut log_files)?;
        self.cleanup_by_total_size(&mut log_files)?;
        self.cleanup_by_max_files(&mut log_files)?;

        Ok(())
    }

    fn cleanup_by_age(&self, log_files: &mut Vec<PathBuf>) -> std::io::Result<()> {
        if log_files.len() <= 1 {
            return Ok(());
        }

        let now = SystemTime::now();
        let max_age = Duration::from_secs(self.config.max_age_hours.saturating_mul(60) * 60);

        let mut kept: Vec<PathBuf> = Vec::new();
        let mut removed_any = false;

        // Keep at least one file (the newest) even if it’s older than max_age.
        let newest_idx = log_files.len().saturating_sub(1);

        for (idx, path) in log_files.iter().enumerate() {
            if idx == newest_idx {
                kept.push(path.clone());
                continue;
            }

            let metadata = match fs::metadata(path) {
                Ok(m) => m,
                Err(_) => {
                    // If we can’t stat it, don’t delete it.
                    kept.push(path.clone());
                    continue;
                }
            };

            let modified = match metadata.modified() {
                Ok(t) => t,
                Err(_) => {
                    kept.push(path.clone());
                    continue;
                }
            };

            let age = match now.duration_since(modified) {
                Ok(d) => d,
                Err(_) => Duration::from_secs(0),
            };

            if age > max_age {
                let _ = fs::remove_file(path);
                removed_any = true;
            } else {
                kept.push(path.clone());
            }
        }

        if removed_any {
            // Re-sort by modification time after deletions.
            kept = self.sort_by_mtime_asc(kept);
        }

        *log_files = kept;
        Ok(())
    }

    fn cleanup_by_total_size(&self, log_files: &mut Vec<PathBuf>) -> std::io::Result<()> {
        if log_files.len() <= 1 {
            return Ok(());
        }

        let mut files = log_files.clone();
        let mut total: u64 = self.total_size_bytes(&files);

        while total > self.config.max_total_size_bytes && files.len() > 1 {
            let oldest = files.remove(0);
            let _ = fs::remove_file(&oldest);
            total = self.total_size_bytes(&files);
        }

        *log_files = files;
        Ok(())
    }

    fn cleanup_by_max_files(&self, log_files: &mut Vec<PathBuf>) -> std::io::Result<()> {
        if log_files.len() <= self.config.max_files {
            return Ok(());
        }
        if log_files.len() <= 1 {
            return Ok(());
        }

        // Keep at least one file.
        let max_remove = log_files.len().saturating_sub(1);
        let requested_remove = log_files.len().saturating_sub(self.config.max_files);
        let files_to_remove = std::cmp::min(max_remove, requested_remove);

        for file in log_files.iter().take(files_to_remove) {
            let _ = fs::remove_file(file);
        }

        let kept = log_files.iter().skip(files_to_remove).cloned().collect();
        *log_files = kept;
        Ok(())
    }

    fn total_size_bytes(&self, files: &[PathBuf]) -> u64 {
        let mut total: u64 = 0;
        for path in files {
            let metadata = match fs::metadata(path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            total = total.saturating_add(metadata.len());
        }
        total
    }

    fn sort_by_mtime_asc(&self, mut files: Vec<PathBuf>) -> Vec<PathBuf> {
        files.sort_by_key(|path| {
            let metadata = match fs::metadata(path) {
                Ok(m) => m,
                Err(_) => return SystemTime::UNIX_EPOCH,
            };
            match metadata.modified() {
                Ok(t) => t,
                Err(_) => SystemTime::UNIX_EPOCH,
            }
        });
        files
    }

    /// Find all log files matching the given prefix, sorted by modification time (oldest first).
    fn find_log_files(&self, log_dir: &Path, log_prefix: &str) -> std::io::Result<Vec<PathBuf>> {
        let mut files: Vec<PathBuf> = Vec::new();

        if !log_dir.exists() {
            return Ok(files);
        }

        for entry in fs::read_dir(log_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() {
                let file_name = match path.file_name() {
                    Some(n) => n,
                    None => continue,
                };

                let file_name_str = file_name.to_string_lossy();
                if file_name_str.starts_with(log_prefix) {
                    files.push(path);
                }
            }
        }

        Ok(self.sort_by_mtime_asc(files))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_rotation_manager_creation() {
        let config = LogRotationConfig::default();
        let manager = LogRotationManager::new(config.clone());
        assert_eq!(manager.config.max_files, config.max_files);
        assert_eq!(manager.config.max_age_hours, config.max_age_hours);
        assert_eq!(
            manager.config.max_total_size_bytes,
            config.max_total_size_bytes
        );
    }

    #[test]
    fn test_log_rotation_config_default() {
        let config = LogRotationConfig::default();
        assert_eq!(config.max_files, 512);
        assert_eq!(config.max_age_hours, 60);
        assert_eq!(config.max_total_size_bytes, 5 * 1024 * 1024 * 1024);
    }
}
