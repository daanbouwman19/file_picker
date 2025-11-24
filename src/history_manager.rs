// src/history_manager.rs

use crate::file_utils::get_history_path;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{self, BufReader, BufWriter},
    path::Path,
};

/// Represents an entry in the video picking history log.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    /// The full path to the video file that was picked.
    pub path: String,
    /// The UTC timestamp indicating when the video was picked.
    pub picked_at: DateTime<Utc>,
}

/// Loads the video picking history from the JSON file.
/// If `custom_path` is provided, it uses that file instead of the default history file.
/// If the file doesn't exist, an empty `Vec` is returned.
/// If the file exists but cannot be parsed, a warning is logged, and an empty `Vec` is returned.
///
/// # Errors
///
/// Returns an error if the history file path cannot be determined or if an
/// I/O error (other than `NotFound`) occurs while reading the file.
pub fn load_history(custom_path: Option<&Path>) -> Result<Vec<HistoryEntry>, Box<dyn std::error::Error>> {
    let history_path_buf;
    let history_path = match custom_path {
        Some(p) => p,
        None => {
            history_path_buf = get_history_path()?;
            &history_path_buf
        }
    };

    match File::open(history_path) {
        Ok(file) => {
            let reader = BufReader::new(file);
            match serde_json::from_reader(reader) {
                Ok(history) => Ok(history),
                Err(e) => {
                    eprintln!(
                        "Warning: Could not parse history file at '{}' (Error: {}). Starting with empty history.",
                        history_path.display(), e
                    );
                    Ok(Vec::new()) // Return empty history on parsing error.
                }
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            Ok(Vec::new()) // File not found is not an error; return empty history.
        }
        Err(e) => {
            Err(Box::new(e)) // Propagate other I/O errors.
        }
    }
}

/// Adds a video file path to the history and saves the updated history to disk.
/// The history is maintained in sorted order by timestamp (most recent first).
/// If `custom_path` is provided, it saves to that file instead of the default history file.
///
/// # Arguments
///
/// * `history` - A mutable reference to the current vector of history entries.
/// * `file_path` - The path of the video file to be added to the history.
///
/// # Errors
///
/// Returns an error if the history file path cannot be determined, or if
/// I/O or serialization errors occur during the saving process.
pub fn add_to_history(
    history: &mut Vec<HistoryEntry>,
    file_path: &Path,
    custom_path: Option<&Path>
) -> Result<(), Box<dyn std::error::Error>> {
    let entry = HistoryEntry {
        path: file_path.to_string_lossy().into_owned(), // Handle potentially non-UTF8 paths.
        picked_at: Utc::now(),
    };
    history.push(entry);
    history.sort_by(|a, b| b.picked_at.cmp(&a.picked_at)); // Sort by timestamp, descending.

    let history_path_buf;
    let history_path = match custom_path {
        Some(p) => p,
        None => {
            history_path_buf = get_history_path()?;
            &history_path_buf
        }
    };

    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(history_path)?;
    let writer = BufWriter::new(file); // Use BufWriter for potentially better I/O performance.
    serde_json::to_writer_pretty(writer, history)?; // Use pretty printing for readability.

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use std::path::PathBuf;

    #[test]
    fn test_add_and_load_history() {
        let temp_file = NamedTempFile::new().unwrap();
        let temp_path = temp_file.path();

        let mut history = Vec::new();
        let video_path = PathBuf::from("/path/to/video.mp4");

        // Add to history
        add_to_history(&mut history, &video_path, Some(temp_path)).unwrap();

        assert_eq!(history.len(), 1);
        assert_eq!(history[0].path, "/path/to/video.mp4");

        // Load history
        let loaded_history = load_history(Some(temp_path)).unwrap();
        assert_eq!(loaded_history.len(), 1);
        assert_eq!(loaded_history[0].path, "/path/to/video.mp4");
        // We can't strictly compare timestamps as serialization might lose precision or time might pass,
        // but since we loaded what we just saved, they should be identical structs.
        assert_eq!(history[0].picked_at, loaded_history[0].picked_at);
    }

    #[test]
    fn test_load_history_empty() {
        let _temp_file = NamedTempFile::new().unwrap();
        // Don't write anything, just an empty file (or rather, just created)
        // Actually NamedTempFile creates an empty file.
        // load_history expects valid JSON or NotFound.
        // If file exists but is empty, serde might fail.
        // Let's test NotFound case by using a path that doesn't exist.

        let temp_dir = tempfile::tempdir().unwrap();
        let non_existent_path = temp_dir.path().join("history.json");

        let history = load_history(Some(&non_existent_path)).unwrap();
        assert!(history.is_empty());
    }
}
