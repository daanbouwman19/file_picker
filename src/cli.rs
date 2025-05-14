// src/cli.rs

use clap::Parser;

/// Defines command-line arguments using the clap crate.
#[derive(Parser, Debug)]
#[clap(
    author,
    version,
    about = "Picks a random video from a folder. Scans recursively by default.",
    long_about = None
)]
pub struct Cli {
    /// Optional path to the video folder. Supports `~` for the home directory.
    /// If omitted, the user will be prompted to enter a path.
    #[clap(short, long)]
    pub folder: Option<String>,

    /// Disables recursive scanning of the video folder.
    /// If this flag is present, only the top-level directory will be scanned.
    #[clap(long, action = clap::ArgAction::SetTrue)]
    pub non_recursive: bool,
}