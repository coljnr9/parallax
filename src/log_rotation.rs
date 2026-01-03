use chrono::Local;
use std::fs;
use std::path::{Path, PathBuf};

/// Configuration for log file rotation
#[derive(Clone, Debug)]
pub struct LogRotationConfig {
    /// Maximum size of a single log file in bytes (default: 100MB)
    pub max_file_size: u64,
    /// Maximum number of rotated log files to keep (default: 10)
    pub max_files: usize,
}

impl Default for LogRotationConfig {
    fn default() -> Self {
        Self {
            max_file_size: 100 * 1024 * 1024, // 100MB
            max_files: 10,
        }
    }
}

/// Manages log file rotation based on size and retention policy
pub struct LogRotationManager {
    config: LogRotationConfig,
}

impl LogRotationManager {
    pub fn new(config: LogRotationConfig) -> Self {
        Self { config }
    }

    /// Check if log rotation is needed and perform cleanup if necessary
    pub fn check_and_rotate(&self, log_dir: &Path, log_prefix: &str) -> std::io::Result<()> {
        // Find all log files matching the pattern
        let log_files = self.find_log_files(log_dir, log_prefix)?;

        if log_files.is_empty() {
            return Ok(());
        }

        // Check the size of the most recent log file
        let latest_log = &log_files[log_files.len() - 1];
        let metadata = fs::metadata(latest_log)?;

        if metadata.len() > self.config.max_file_size {
            // Rotate the log file by renaming it with a timestamp
            let timestamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
            let stem = match latest_log.file_stem() {
                Some(s) => s.to_string_lossy(),
                None => std::borrow::Cow::Borrowed(""),
            };
            let rotated_name = format!("{}.{}", stem, timestamp);
            let parent = match latest_log.parent() {
                Some(p) => p,
                None => log_dir,
            };
            let rotated_path = parent.join(rotated_name);

            fs::rename(latest_log, &rotated_path)?;
        }

        // Clean up old log files if we exceed the max_files limit
        let log_files = self.find_log_files(log_dir, log_prefix)?;
        if log_files.len() > self.config.max_files {
            let files_to_remove = log_files.len() - self.config.max_files;
            for file in log_files.iter().take(files_to_remove) {
                let _ = fs::remove_file(file);
            }
        }

        Ok(())
    }

    /// Find all log files matching the given prefix, sorted by modification time
    fn find_log_files(&self, log_dir: &Path, log_prefix: &str) -> std::io::Result<Vec<PathBuf>> {
        let mut files = Vec::new();

        if !log_dir.exists() {
            return Ok(files);
        }

        for entry in fs::read_dir(log_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() {
                if let Some(file_name) = path.file_name() {
                    let file_name_str = file_name.to_string_lossy();
                    if file_name_str.starts_with(log_prefix) {
                        files.push(path);
                    }
                }
            }
        }

        // Sort by modification time (oldest first)
        files.sort_by_key(|path| {
            let metadata = match fs::metadata(path) {
                Ok(m) => m,
                Err(_) => return std::time::SystemTime::UNIX_EPOCH,
            };
            match metadata.modified() {
                Ok(t) => t,
                Err(_) => std::time::SystemTime::UNIX_EPOCH,
            }
        });

        Ok(files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_rotation_manager_creation() {
        let config = LogRotationConfig::default();
        let manager = LogRotationManager::new(config);
        assert_eq!(manager.config.max_file_size, 100 * 1024 * 1024);
        assert_eq!(manager.config.max_files, 10);
    }

    #[test]
    fn test_log_rotation_config_default() {
        let config = LogRotationConfig::default();
        assert_eq!(config.max_file_size, 100 * 1024 * 1024);
        assert_eq!(config.max_files, 10);
    }
}
