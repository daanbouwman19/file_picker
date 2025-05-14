// src/config.rs

/// A list of recognized video file extensions (all lowercase).
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "webm", "flv", "wmv", "mpg", "mpeg", "m4v",
];
/// The filename for storing the history of picked videos.
pub const HISTORY_FILE_NAME: &str = "history.json";
/// The application name, used for creating the application-specific data directory.
pub const APP_NAME: &str = "random_video_picker";

/// The name of the ffprobe executable, which is platform-dependent.
#[cfg(windows)]
pub const FFPROBE_EXECUTABLE_NAME: &str = "ffprobe.exe";
#[cfg(not(windows))]
pub const FFPROBE_EXECUTABLE_NAME: &str = "ffprobe";