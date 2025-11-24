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
    // Sort for deterministic output in tests and UI
    video_files.sort();
    Ok(video_files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs::File;

    #[test]
    fn test_find_video_files_recursive() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        // Create files
        File::create(root.join("video1.mp4")).unwrap();
        File::create(root.join("image.jpg")).unwrap();

        let subdir = root.join("subdir");
        fs::create_dir(&subdir).unwrap();
        File::create(subdir.join("video2.mkv")).unwrap();

        let files = find_video_files(root, true).unwrap();
        assert_eq!(files.len(), 2);

        let file_names: Vec<String> = files.iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();

        assert!(file_names.contains(&"video1.mp4".to_string()));
        assert!(file_names.contains(&"video2.mkv".to_string()));
    }

    #[test]
    fn test_find_video_files_non_recursive() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        // Create files
        File::create(root.join("video1.mp4")).unwrap();

        let subdir = root.join("subdir");
        fs::create_dir(&subdir).unwrap();
        File::create(subdir.join("video2.mkv")).unwrap();

        let files = find_video_files(root, false).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_name().unwrap().to_string_lossy(), "video1.mp4");
    }

    #[test]
    fn test_find_video_files_invalid_dir() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("file.txt");
        File::create(&file_path).unwrap();

        let result = find_video_files(&file_path, true);
        assert!(result.is_err());
    }
}
