// src/video_entry.rs

use std::path::PathBuf;

/// Represents a video file discovered during scanning.
/// Includes its path and how many times it has been picked, used for weighting selection.
#[derive(Debug, Clone)]
pub struct VideoEntry {
    /// The full path to the video file.
    pub path: PathBuf,
    /// The number of times this video has been recorded in the history.
    pub pick_count: usize,
}

impl VideoEntry {
    /// Creates a new `VideoEntry`.
    ///
    /// # Arguments
    ///
    /// * `path` - The `PathBuf` for the video file.
    /// * `pick_count` - How many times this video has been picked previously.
    pub fn new(path: PathBuf, pick_count: usize) -> Self {
        VideoEntry { path, pick_count }
    }

    /// Calculates the selection weight for this video entry.
    /// The weight is inversely proportional to (pick_count + 1).
    /// Videos picked fewer times have a higher weight.
    ///
    /// Examples:
    /// * `pick_count` = 0 => weight = 1.0 / (0 + 1) = 1.0
    /// * `pick_count` = 1 => weight = 1.0 / (1 + 1) = 0.5
    /// * `pick_count` = 2 => weight = 1.0 / (2 + 1) = 0.333...
    ///
    /// Returns the weight as `f64`.
    pub fn weight(&self) -> f64 {
        // Adding 1.0 ensures unpicked items (pick_count = 0) have a weight of 1.0
        // and avoids division by zero (though pick_count is usize).
        1.0 / (self.pick_count as f64 + 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_weight_calculation() {
        let entry_0 = VideoEntry::new(PathBuf::from("video1.mp4"), 0);
        assert!((entry_0.weight() - 1.0).abs() < f64::EPSILON);

        let entry_1 = VideoEntry::new(PathBuf::from("video2.mp4"), 1);
        assert!((entry_1.weight() - 0.5).abs() < f64::EPSILON);

        let entry_3 = VideoEntry::new(PathBuf::from("video3.mp4"), 3);
        assert!((entry_3.weight() - 0.25).abs() < f64::EPSILON);
    }
}
