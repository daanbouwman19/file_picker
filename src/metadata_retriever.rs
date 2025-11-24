// src/metadata_retriever.rs

use crate::config::FFPROBE_EXECUTABLE_NAME;
use serde::Deserialize;
use std::{
    env,
    fmt,
    io::Error as IoError,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Custom error type for ffprobe command execution failures.
#[derive(Debug)]
pub struct FfprobeError {
    message: String,
    source: Option<Box<dyn std::error::Error + 'static>>,
}

impl FfprobeError {
    /// Creates a new FfprobeError with just a message.
    pub fn new<S>(message: S) -> Self 
    where
        S: Into<String>,
    {
        Self { 
            message: message.into(),
            source: None,
        }
    }

    /// Creates a new FfprobeError with a message and source error.
    pub fn with_source<S, E>(message: S, source: E) -> Self 
    where
        S: Into<String>,
        E: std::error::Error + 'static,
    {
        Self {
            message: message.into(),
            source: Some(Box::new(source)),
        }
    }
}

impl fmt::Display for FfprobeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ffprobe error: {}", self.message)
    }
}

impl std::error::Error for FfprobeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_deref()
    }
}

/// Stores extracted metadata for a video file, such as resolution and duration.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VideoMetadata {
    /// Video resolution (e.g., "1920x1080"). Optional.
    pub resolution: Option<String>,
    /// Video duration (e.g., "01:23:45" or "MM:SS"). Optional.
    pub duration: Option<String>,
}

// Internal structs for parsing ffprobe JSON output.
#[derive(Deserialize, Debug)]
struct FfprobeOutput {
    #[serde(default)] // Handles cases where 'streams' might be missing.
    streams: Vec<FfprobeStream>,
    format: FfprobeFormat,
}

#[derive(Deserialize, Debug)]
struct FfprobeStream {
    codec_type: Option<String>, // e.g., "video", "audio".
    width: Option<i64>,
    height: Option<i64>,
    duration: Option<String>, // Duration in seconds (string format), per stream.
}

#[derive(Deserialize, Debug)]
struct FfprobeFormat {
    duration: Option<String>, // Overall duration in seconds (string format).
}

/// Formats a duration string (representing seconds) into HH:MM:SS or MM:SS.
/// Returns `None` if the input string cannot be parsed as a non-negative float.
fn format_duration_string(duration_str: &str) -> Option<String> {
    match duration_str.trim().parse::<f64>() {
        Ok(secs_float) => {
            if secs_float < 0.0 {
                eprintln!(
                    "Warning: Parsed negative duration '{}'. Ignoring.",
                    duration_str
                );
                return None;
            }
            let secs = secs_float.round() as u64;
            let hours = secs / 3600;
            let minutes = (secs % 3600) / 60;
            let seconds = secs % 60;
            if hours > 0 {
                Some(format!("{:02}:{:02}:{:02}", hours, minutes, seconds))
            } else {
                Some(format!("{:02}:{:02}", minutes, seconds))
            }
        }
        Err(_) => {
            eprintln!(
                "Warning: Could not parse duration string '{}' as float.",
                duration_str
            );
            None
        }
    }
}

/// Parses the raw JSON output from ffprobe into `VideoMetadata`.
///
/// This function is extracted to facilitate unit testing without needing the `ffprobe` executable.
///
/// # Arguments
///
/// * `json_str` - The JSON output string from ffprobe.
///
/// # Errors
///
/// Returns an error if the JSON cannot be parsed.
pub fn parse_ffprobe_output(json_str: &str) -> Result<VideoMetadata, Box<dyn std::error::Error>> {
    let parsed_data: FfprobeOutput = serde_json::from_str(json_str).map_err(|e| {
        eprintln!(
            "Failed to parse JSON output: {}. Raw output:\n---\n{}\n---",
            e, json_str
        );
        FfprobeError::with_source("failed to parse JSON output", e)
    })?;

    let mut video_info = VideoMetadata::default();

    if let Some(stream) = parsed_data
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"))
    {
        if let (Some(width), Some(height)) = (stream.width, stream.height) {
            if width > 0 && height > 0 {
                video_info.resolution = Some(format!("{}x{}", width, height));
            }
        }
    }

    // Prefer format.duration, fallback to video stream duration.
    let duration_str_opt = parsed_data.format.duration.as_ref().or_else(|| {
        parsed_data
            .streams
            .iter()
            .find(|s| s.codec_type.as_deref() == Some("video"))
            .and_then(|s| s.duration.as_ref())
    });

    if let Some(duration_str) = duration_str_opt {
        video_info.duration = format_duration_string(duration_str);
    }

    Ok(video_info)
}

/// Retrieves video metadata by running `ffprobe`.
///
/// `ffprobe` is searched in the following locations:
/// 1. Next to the application executable.
/// 2. In a `tools` subdirectory next to the executable.
/// 3. In `CARGO_MANIFEST_DIR/tools` (debug builds only, for development).
/// 4. In the system's PATH.
///
/// # Arguments
///
/// * `file_path` - Path to the video file.
///
/// # Errors
///
/// Returns an error if `ffprobe` execution fails, or its output cannot be parsed.
pub fn get_video_metadata(file_path: &Path) -> Result<VideoMetadata, Box<dyn std::error::Error>> {
    // --- Locate ffprobe executable ---
    let mut ffprobe_command_path = PathBuf::from(FFPROBE_EXECUTABLE_NAME); // Default to PATH.
    if let Ok(current_exe_path) = env::current_exe() {
        if let Some(exe_dir) = current_exe_path.parent() {
            let paths_to_check = [
                exe_dir.join(FFPROBE_EXECUTABLE_NAME),
                exe_dir.join("tools").join(FFPROBE_EXECUTABLE_NAME),
            ];
            let mut found_path: Option<PathBuf> = None;
            for path_to_check in paths_to_check {
                if path_to_check.is_file() {
                    found_path = Some(path_to_check);
                    break;
                }
            }

            // Dev environment check (debug builds only): project_root/tools/ffprobe[.exe]
            if found_path.is_none() && cfg!(debug_assertions) {
                if let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") {
                    let project_tools_path = PathBuf::from(manifest_dir)
                        .join("tools")
                        .join(FFPROBE_EXECUTABLE_NAME);
                    if project_tools_path.is_file() {
                        found_path = Some(project_tools_path);
                    }
                }
            }

            if let Some(path) = found_path {
                ffprobe_command_path = path; // Use specifically found path.
            }
        }
    }

    // --- Execute ffprobe ---
    let output = Command::new(&ffprobe_command_path)
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(file_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| {
            IoError::new(
                e.kind(),
                format!(
                    "Failed to execute ffprobe command '{}': {}",
                    ffprobe_command_path.display(),
                    e
                ),
            )
        })?;

    // --- Check ffprobe Exit Status ---
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!(
            "ffprobe command failed (status: {}). Stderr: {}",
            output.status, stderr
        );
        return Err(Box::new(FfprobeError::new(
            format!("command failed (status: {}): {}", output.status, stderr.trim()),
        )));
    }

    // --- Parse ffprobe JSON Output ---
    let json_str = String::from_utf8_lossy(&output.stdout);
    parse_ffprobe_output(&json_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration_string() {
        assert_eq!(format_duration_string("100.5"), Some("01:41".to_string()));
        assert_eq!(format_duration_string("3665"), Some("01:01:05".to_string()));
        assert_eq!(format_duration_string("0"), Some("00:00".to_string()));
        assert_eq!(format_duration_string("-5"), None); // Should return None and warn
        assert_eq!(format_duration_string("invalid"), None);
    }

    #[test]
    fn test_parse_ffprobe_output_full() {
        let json = r#"
        {
            "streams": [
                {
                    "codec_type": "video",
                    "width": 1920,
                    "height": 1080,
                    "duration": "100.0"
                },
                {
                    "codec_type": "audio"
                }
            ],
            "format": {
                "duration": "100.0"
            }
        }
        "#;
        let metadata = parse_ffprobe_output(json).unwrap();
        assert_eq!(metadata.resolution, Some("1920x1080".to_string()));
        assert_eq!(metadata.duration, Some("01:40".to_string()));
    }

    #[test]
    fn test_parse_ffprobe_output_missing_video_stream() {
        let json = r#"
        {
            "streams": [
                {
                    "codec_type": "audio"
                }
            ],
            "format": {
                "duration": "50.0"
            }
        }
        "#;
        let metadata = parse_ffprobe_output(json).unwrap();
        assert_eq!(metadata.resolution, None);
        assert_eq!(metadata.duration, Some("00:50".to_string()));
    }

    #[test]
    fn test_parse_ffprobe_output_malformed_json() {
        let json = r#" { "invalid": } "#;
        assert!(parse_ffprobe_output(json).is_err());
    }
}
