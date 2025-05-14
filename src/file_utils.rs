// src/file_utils.rs

use crate::config::{APP_NAME, HISTORY_FILE_NAME, VIDEO_EXTENSIONS};
use std::{
    fs,
    io::{Error as IoError, ErrorKind as IoErrorKind},
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

/// Returns the full path to the application's data directory.
/// This function creates the directory if it does not already exist.
///
/// # Errors
///
/// Returns an error if the system's data directory cannot be determined
/// or if creating the application data directory fails.
pub fn get_app_data_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let data_dir_base = dirs::data_dir().ok_or_else(|| {
        IoError::new(
            IoErrorKind::NotFound,
            "Failed to determine the system's data directory.",
        )
    })?;

    let app_data_dir = data_dir_base.join(APP_NAME);

    // fs::create_dir_all is idempotent; it does nothing if the directory already exists.
    fs::create_dir_all(&app_data_dir)?;

    Ok(app_data_dir)
}

/// Returns the full path to the history JSON file, located within the app data directory.
///
/// # Errors
///
/// Returns an error if the application data directory cannot be determined.
pub fn get_history_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(get_app_data_dir()?.join(HISTORY_FILE_NAME))
}

/// Scans the specified folder for files with recognized video extensions.
/// The scan can be performed recursively.
///
/// # Arguments
///
/// * `folder_path` - The path to the directory to be scanned.
/// * `recursive` - If true, subdirectories are scanned; otherwise, only the top-level directory is scanned.
///
/// # Errors
///
/// Returns an error if:
/// * `folder_path` is not a valid directory.
/// * An issue occurs while accessing files or directories during the scan.
pub fn find_video_files(
    folder_path: &Path,
    recursive: bool,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    if !folder_path.is_dir() {
        return Err(Box::new(IoError::new(
            IoErrorKind::InvalidInput,
            format!("Path is not a directory: {}", folder_path.display()),
        )));
    }

    let mut video_files = Vec::new();

    let walker = WalkDir::new(folder_path).min_depth(1); // Start scanning from depth 1 (contents of the folder)
    let walker = if recursive {
        walker // No max_depth results in a fully recursive scan.
    } else {
        walker.max_depth(1) // Limit scan to the top-level directory contents. Note: Original had max_depth(5) - adjusted to 1 for true non-recursive.
    };

    for entry_result in walker {
        let entry = entry_result?; // Propagate errors encountered during directory walking.
        let path = entry.path();

        if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if VIDEO_EXTENSIONS.contains(&ext.to_lowercase().as_str()) {
                    video_files.push(path.to_path_buf());
                }
            }
        }
    }
    Ok(video_files)
}