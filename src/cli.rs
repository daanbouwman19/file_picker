// src/cli.rs

use clap::Parser;

#[derive(Parser, Debug)]
#[clap(
    author,
    version,
    about = "Picks a random video from a folder. Scans recursively by default.",
    long_about = None
)]
pub struct Cli {
    #[clap(short, long)]
    pub folder: Option<String>,

    #[clap(long, action = clap::ArgAction::SetTrue)]
    pub non_recursive: bool,

    /// Disable the remote streaming server entirely.
    #[clap(long, name = "no-streaming", action = clap::ArgAction::SetTrue)]
    pub no_streaming: bool,
}