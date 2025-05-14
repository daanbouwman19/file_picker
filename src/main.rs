// src/main.rs

use actix_web::web;
use clap::Parser;
use dialoguer::{theme::ColorfulTheme, Input, Select};
use local_ip_address::local_ip;
use qrcode::render::unicode;
use qrcode::QrCode;
use rand::prelude::*;
use std::env;
use std::path::PathBuf;
use std::process;
use std::sync::{Arc, Mutex};

// Module declarations
mod cli;
mod config;
mod file_utils;
mod history_manager;
mod metadata_retriever;
mod stream_server;
mod ui;
mod video_entry;

// Crate imports for convenience
use crate::cli::Cli;
use crate::file_utils::find_video_files;
use crate::history_manager::{add_to_history, load_history, HistoryEntry};
use crate::metadata_retriever::get_video_metadata;
use crate::stream_server::{run_server, StreamState};
use crate::ui::view_history;
use crate::video_entry::VideoEntry;

const STREAMING_PORT: u16 = 8080;

#[tokio::main]
async fn main() {
    if let Err(err) = run_app().await {
        eprintln!("\nApplication Error: {}", err);
        process::exit(1);
    }
}

async fn run_app() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok(); // Load .env file if present, ignore if not.

    let cli_args = Cli::parse();
    let theme = ColorfulTheme::default();
    let mut history: Vec<HistoryEntry> = load_history()?;

    // Shared state for the video path to be streamed.
    let stream_state_inner: StreamState = Arc::new(Mutex::new(None));
    let app_state = web::Data::new(stream_state_inner.clone());

    let local_ip_addr = local_ip()?.to_string();
    let stream_url_for_display = format!("http://{}:{}/stream", local_ip_addr, STREAMING_PORT);

    // Spawn the Actix web server in a separate Tokio task.
    let server = run_server(local_ip_addr.clone(), STREAMING_PORT, app_state.clone())?;
    tokio::spawn(server);

    // Determine initial folder path from CLI args or environment variable.
    let mut current_folder_path: Option<PathBuf> = cli_args
        .folder
        .map(|s| PathBuf::from(shellexpand::tilde(&s).into_owned()))
        .or_else(|| {
            env::var("DEFAULT_VIDEO_FOLDER")
                .ok()
                .map(|s| PathBuf::from(shellexpand::tilde(&s).into_owned()))
        });
    let scan_recursively = !cli_args.non_recursive;

    // Main application loop
    loop {
        let folder_path = match current_folder_path.clone() {
            Some(path) => path,
            None => PathBuf::from(
                shellexpand::full(
                    &Input::<String>::with_theme(&theme)
                        .with_prompt("Enter the path to the video folder (use ~ for home)")
                        .interact_text()?,
                )?
                .into_owned(),
            ),
        };

        let video_files_paths = match find_video_files(&folder_path, scan_recursively) {
            Ok(files) => files,
            Err(e) => {
                eprintln!("Error scanning folder '{}': {}", folder_path.display(), e);
                current_folder_path = None; // Reset folder path to re-prompt.
                continue;
            }
        };

        if video_files_paths.is_empty() {
            println!("No video files found in {}", folder_path.display());
            let action = Select::with_theme(&theme)
                .with_prompt("No videos found. What would you like to do?")
                .items(&["Choose another folder", "View history", "Quit"])
                .default(0)
                .interact_opt()?
                .unwrap_or(2); // Default to Quit if Esc is pressed.

            match action {
                0 => current_folder_path = None, // Prompt for a new folder.
                1 => {
                    view_history(&history, &theme)?;
                    current_folder_path = None; // Still prompt for a new folder after viewing history.
                }
                _ => break, // Quit the application.
            }
            continue;
        }

        // Create VideoEntry instances with pick counts from history.
        let video_entries: Vec<VideoEntry> = video_files_paths
            .into_iter()
            .map(|path| {
                let pick_count = history
                    .iter()
                    .filter(|h_entry| h_entry.path == path.to_string_lossy())
                    .count();
                VideoEntry::new(path, pick_count)
            })
            .collect();

        // Select a video based on weighted choice.
        let selected_video_entry = video_entries
            .choose_weighted(&mut rand::rng(), |item| item.weight())?
            .clone();
        let selected_file = selected_video_entry.path.clone();

        println!(
            "\n✨ Picked: {} (Pick count: {})",
            selected_file.display(),
            selected_video_entry.pick_count
        );

        // Attempt to retrieve and display video metadata.
        if let Ok(metadata) = get_video_metadata(&selected_file) {
            println!(
                "Resolution: {}, Duration: {}",
                metadata.resolution.unwrap_or_else(|| "N/A".into()),
                metadata.duration.unwrap_or_else(|| "N/A".into())
            );
        }

        add_to_history(&mut history, &selected_file)?;

        // Update the shared state for the streaming server.
        {
            let mut state_guard = stream_state_inner.lock().unwrap();
            *state_guard = Some(selected_file.clone());
        }

        println!("Streaming URL: {}", stream_url_for_display);

        // Display QR code for the streaming URL.
        if let Ok(code) = QrCode::new(stream_url_for_display.as_bytes()) {
            println!("{}", code.render::<unicode::Dense1x2>().build());
        }

        let actions = &[
            "Pick another from the same folder",
            "Choose a different folder",
            "View history",
            "Quit",
        ];

        let choice = Select::with_theme(&theme)
            .with_prompt("What next?")
            .items(actions)
            .default(0)
            .interact_opt()?
            .unwrap_or(3); // Default to Quit.

        match choice {
            0 => current_folder_path = Some(folder_path.clone()), // Keep current folder.
            1 => current_folder_path = None, // Clear folder to prompt for new one.
            2 => {
                view_history(&history, &theme)?;
                current_folder_path = Some(folder_path.clone()); // Keep current folder after viewing history.
            }
            _ => break, // Quit.
        }
    }

    println!("Goodbye!");
    Ok(())
}
