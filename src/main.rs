// src/main.rs

use actix_web::{dev::ServerHandle, web}; // Added ServerHandle for graceful shutdown
use clap::Parser;
use dialoguer::{theme::ColorfulTheme, Input, Select};
use local_ip_address::local_ip;
use qrcode::render::unicode;
use qrcode::QrCode;
use rand::prelude::*;
use std::env;
use std::path::{Path, PathBuf}; // Added Path import
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
use crate::stream_server::{run_server, StreamState}; // StreamState is Arc<Mutex<Option<PathBuf>>>
use crate::ui::view_history;
use crate::video_entry::VideoEntry;

const STREAMING_PORT: u16 = 8080;

/// Attempts to open the given video file path with the system's default application.
///
/// # Arguments
///
/// * `video_path` - A reference to the `Path` of the video file to be played.
///
/// # Returns
///
/// * `Ok(())` if the command to open the video was successfully dispatched.
/// * `Err(Box<dyn std::error::Error>)` if there was an error trying to open the video.
fn play_video_locally(video_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    match open::that(video_path) {
        Ok(_) => {
            println!(
                "Attempting to open '{}' with the default system player.",
                video_path.display()
            );
            Ok(())
        }
        Err(e) => Err(format!(
            "Failed to open video locally with system handler for '{}': {}",
            video_path.display(),
            e
        )
        .into()),
    }
}

#[tokio::main]
async fn main() {
    if let Err(err) = run_app().await {
        eprintln!("\nApplication Error: {}", err);
        process::exit(1);
    }
}

async fn run_app() -> Result<(), Box<dyn std::error::Error>> {
    // Load environment variables from .env file, if present.
    dotenvy::dotenv().ok();

    // Parse command-line arguments.
    let cli_args = Cli::parse();
    let theme = ColorfulTheme::default();
    let mut history: Vec<HistoryEntry> = load_history()?;

    // --- Streaming Server Setup ---
    // These variables will hold the server's control handle, the shared state for the video path,
    // and the base URL for streaming, if streaming is enabled.
    let mut actix_server_main_handle: Option<ServerHandle> = None;
    let mut stream_state_arc: Option<StreamState> = None; // StreamState is Arc<Mutex<Option<PathBuf>>>
    let mut stream_url_base: Option<String> = None;

    // Start the streaming server unless the --no-streaming flag is present.
    if !cli_args.no_streaming {
        let local_ip_addr = match local_ip() {
            Ok(ip) => ip.to_string(),
            Err(e) => {
                eprintln!(
                    "Warning: Could not get local IP address for streaming: {}. Streaming will be disabled.",
                    e
                );
                String::new() // Use an empty string to signify failure to get IP.
            }
        };

        // Proceed with server setup only if a local IP was successfully obtained.
        if !local_ip_addr.is_empty() {
            stream_url_base = Some(format!("http://{}:{}", local_ip_addr, STREAMING_PORT));
            
            // Create the shared state for the streaming server.
            let state_for_server_instance = Arc::new(Mutex::new(None::<PathBuf>));
            stream_state_arc = Some(state_for_server_instance.clone()); // Keep a reference to update the path later.

            // Prepare the application state for Actix.
            let app_state_for_server_config = web::Data::new(state_for_server_instance);
            
            // Attempt to run the server.
            match run_server(local_ip_addr.clone(), STREAMING_PORT, app_state_for_server_config) {
                Ok(server) => {
                    actix_server_main_handle = Some(server.handle()); // Store the server handle for graceful shutdown.
                    tokio::spawn(server); // Run the server in a separate Tokio task.
                    println!("Streaming server is running at {}:{}", local_ip_addr, STREAMING_PORT);
                }
                Err(e) => {
                    eprintln!("Failed to start streaming server: {}. Streaming will be disabled.", e);
                    // Reset streaming-related variables if server startup fails.
                    stream_state_arc = None;
                    stream_url_base = None;
                }
            }
        } else if !cli_args.no_streaming { // Explicitly inform if IP was the issue but --no-streaming wasn't used.
             println!("Streaming disabled: Could not determine local IP address.");
        }
    } else {
        println!("Streaming server is disabled via the --no-streaming flag.");
    }

    // Determine the initial folder to scan for videos.
    let mut current_folder_path: Option<PathBuf> = cli_args
        .folder
        .map(|s| PathBuf::from(shellexpand::tilde(&s).into_owned())) // Expand `~` if present.
        .or_else(|| {
            env::var("DEFAULT_VIDEO_FOLDER") // Fallback to environment variable.
                .ok()
                .map(|s| PathBuf::from(shellexpand::tilde(&s).into_owned()))
        });
    let scan_recursively = !cli_args.non_recursive;

    // Main application loop: continues until the user quits.
    'outer: loop {
        // Get the folder path from the user if not already set.
        let folder_path = match current_folder_path.clone() {
            Some(path) => path,
            None => PathBuf::from(
                shellexpand::full( // Expand environment variables and `~`.
                    &Input::<String>::with_theme(&theme)
                        .with_prompt("Enter the path to the video folder (supports ~ and env vars)")
                        .interact_text()?,
                )?
                .into_owned(),
            ),
        };

        // Find video files in the specified folder.
        let video_files_paths = match find_video_files(&folder_path, scan_recursively) {
            Ok(files) => files,
            Err(e) => {
                eprintln!("Error scanning folder '{}': {}", folder_path.display(), e);
                current_folder_path = None; // Reset folder path to re-prompt.
                continue 'outer; // Restart the loop to ask for a folder again.
            }
        };

        // Handle the case where no video files are found.
        if video_files_paths.is_empty() {
            println!("No video files found in '{}'.", folder_path.display());
            let action = Select::with_theme(&theme)
                .with_prompt("No videos found. What would you like to do?")
                .items(&["Choose another folder", "View history", "Quit"])
                .default(0)
                .interact_opt()? // `interact_opt` allows for Esc to cancel.
                .unwrap_or(2); // Default to "Quit" if Esc is pressed.

            match action {
                0 => current_folder_path = None, // Prompt for a new folder.
                1 => {
                    view_history(&history, &theme)?;
                    current_folder_path = None; // Still prompt for a new folder after viewing history.
                }
                _ => break 'outer, // Quit the application.
            }
            continue 'outer; // Restart the loop.
        }

        // Create VideoEntry instances, including pick counts from history for weighting.
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

        // Select a video based on weighted choice (favoring less picked videos).
        let selected_video_entry = video_entries
            .choose_weighted(&mut rand::rng(), |item| item.weight())?
            .clone();
        let selected_file = selected_video_entry.path.clone();

        println!(
            "\n✨ Picked: {} (Previously picked {} times)",
            selected_file.display(),
            selected_video_entry.pick_count
        );

        // Attempt to retrieve and display video metadata (resolution, duration).
        if let Ok(metadata) = get_video_metadata(&selected_file) {
            println!(
                "Metadata: Resolution: {}, Duration: {}",
                metadata.resolution.unwrap_or_else(|| "N/A".into()),
                metadata.duration.unwrap_or_else(|| "N/A".into())
            );
        }

        // Add the picked video to history.
        add_to_history(&mut history, &selected_file)?;

        // Inner loop: handles actions for the currently selected video.
        'inner: loop {
            let mut actions = vec!["Play locally"]; // Default action.
            let mut current_video_streaming_url: Option<String> = None;

            // Dynamically add streaming-related actions if the server is running.
            if let Some(state_arc_ref) = &stream_state_arc { // Check if streaming server was intended to start.
                if let Some(base_url_ref) = &stream_url_base { // Check if base URL was successfully formed.
                    let mut is_current_video_set_for_streaming = false;
                    // Check if the *current* selected video is already set for streaming.
                    if let Ok(guard) = state_arc_ref.lock() { // Lock the Mutex to access the shared path.
                        if let Some(streaming_path) = &*guard {
                            if streaming_path == &selected_file {
                                is_current_video_set_for_streaming = true;
                            }
                        }
                    }

                    if is_current_video_set_for_streaming {
                        let url = format!("{}/stream", base_url_ref);
                        actions.push("Get Streaming Link (current video)");
                        current_video_streaming_url = Some(url);
                    } else {
                        actions.push("Stream this video");
                    }
                }
            }
            
            // Add common actions.
            actions.extend(vec![
                "Pick another from this folder",
                "Choose a different folder",
                "View history",
                "Quit",
            ]);

            let choice_prompt = format!(
                "Selected: '{}'. What next?",
                selected_file.file_name().map_or_else(
                    || selected_file.to_string_lossy(), // Fallback to full path if no filename.
                    |name| name.to_string_lossy()      // Use filename if available.
                )
            );
            
            let choice_idx = Select::with_theme(&theme)
                .with_prompt(&choice_prompt)
                .items(&actions)
                .default(0) // Default to "Play locally".
                .interact_opt()?
                // If Esc is pressed, default to the "Quit" option.
                .unwrap_or_else(|| actions.iter().position(|a| *a == "Quit").unwrap_or(actions.len() - 1));


            // Handle the user's choice.
            // The `s` in `.map(|s| *s)` will be `&&str`, so `*s` dereferences it to `&str`.
            match actions.get(choice_idx).map(|s| *s) {
                Some("Play locally") => {
                    if let Err(e) = play_video_locally(&selected_file) {
                        eprintln!("Error playing video locally: {}", e);
                    }
                    // After attempting to play, stay in the inner loop to offer more actions for the same video.
                }
                Some("Stream this video") => {
                    if let (Some(state_arc_ref_update), Some(base_url_ref_display)) = (&stream_state_arc, &stream_url_base) {
                        { // Lock scope for updating the shared state.
                            let mut guard = state_arc_ref_update.lock().unwrap();
                            *guard = Some(selected_file.clone()); // Set the current video for streaming.
                        }
                        let full_stream_url = format!("{}/stream", base_url_ref_display);
                        println!("Streaming URL for '{}': {}", selected_file.display(), full_stream_url);
                        if let Ok(code) = QrCode::new(full_stream_url.as_bytes()) {
                            println!("Scan QR code to stream on another device:\n{}", code.render::<unicode::Dense1x2>().build());
                        }
                    } else {
                        println!("Streaming is not available or was not enabled for this session.");
                    }
                }
                Some("Get Streaming Link (current video)") => {
                    if let Some(url) = &current_video_streaming_url {
                        println!("Streaming URL for '{}': {}", selected_file.display(), url);
                         if let Ok(code) = QrCode::new(url.as_bytes()) {
                            println!("Scan QR code to stream on another device:\n{}", code.render::<unicode::Dense1x2>().build());
                        }
                    } else {
                        // This case should ideally not be reached if UI logic is correct.
                        println!("Streaming link is not available. Try 'Stream this video' first if the option is present.");
                    }
                }
                Some("Pick another from this folder") => {
                    current_folder_path = Some(folder_path.clone()); // Keep current folder.
                    break 'inner; // Exit inner loop, go to 'outer to re-pick from same folder.
                }
                Some("Choose a different folder") => {
                    current_folder_path = None; // Clear folder to prompt for new one.
                    break 'inner; // Exit inner loop, go to 'outer to select new folder.
                }
                Some("View history") => {
                    view_history(&history, &theme)?;
                    // Stay in inner loop for the same video after viewing history.
                }
                Some("Quit") | Some(_) | None => { // Treat unknown action or Esc (None from interact_opt) as Quit.
                    if let Some(server_handle_to_stop) = actix_server_main_handle.take() { // Use .take() to avoid multiple stop calls.
                        println!("\nStopping streaming server...");
                        // The stop method is async; await it.
                        server_handle_to_stop.stop(true).await; 
                        println!("Streaming server stopped.");
                    }
                    println!("Goodbye!");
                    return Ok(()); // Exit the run_app function, terminating the program.
                }
            }
        } // End of 'inner' action loop for the selected video.
    } // End of 'outer' main application loop.

    // Fallback for server shutdown if the loop exits unexpectedly (should not happen with current logic).
    if let Some(server_handle_to_stop) = actix_server_main_handle.take() {
        server_handle_to_stop.stop(true).await;
    }
    Ok(())
}
