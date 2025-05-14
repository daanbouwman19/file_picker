// src/main.rs

use actix_web::{dev::ServerHandle, web};
use clap::Parser;
use dialoguer::{theme::ColorfulTheme, Input, Select};
use local_ip_address::local_ip;
use qrcode::render::unicode;
use qrcode::QrCode;
use rand::prelude::*;
use std::collections::HashMap; // Added for HashMap
use std::env;
use std::path::{Path, PathBuf};
use std::process; // Corrected typo: proces -> process
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

/// Attempts to open a video file with the system's default media player.
///
/// # Arguments
///
/// * `video_path` - A `Path` to the video file to be opened.
///
/// # Returns
///
/// `Ok(())` if the attempt to open the file was successful (note: this doesn't guarantee the player launched correctly, only that the OS command was issued).
/// `Err` with a descriptive error message if `open::that` fails.
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
            "Failed to open video locally with system handler for '{}': {}\nUnderlying error: {}",
            video_path.display(),
            e,
            e
        )
        .into()),
    }
}

// Enum to control the flow of the main loop when no videos are found
enum LoopControl {
    Continue,
    Break,
}

#[tokio::main]
async fn main() {
    if let Err(err) = run_app().await {
        eprintln!("\nApplication Error: {}", err);
        process::exit(1);
    }
}

/// Handles the user interaction flow when no video files are found in the scanned directory.
///
/// It prompts the user to choose an action:
/// - Choose another folder: Resets `current_folder_path` and `cached_folder_scan`.
/// - View history: Displays the video history.
/// - Quit: Signals to break the main application loop.
///
/// # Arguments
///
/// * `folder_path_display` - A string slice representing the path of the folder where no videos were found, for display purposes.
/// * `theme` - The `ColorfulTheme` for `dialoguer` prompts. Passed as `&str` for efficient display within the function.
/// * `history` - A slice of `HistoryEntry` to be passed to `view_history` if selected.
/// * `current_folder_path` - A mutable reference to `Option<PathBuf>`, potentially set to `None` if the user chooses to select a new folder.
/// * `cached_folder_scan` - A mutable reference to the folder scan cache, potentially set to `None`.
///
/// # Returns
///
/// `Ok(LoopControl)` indicating whether the main loop should `Continue` or `Break`.
/// `Err` if any `dialoguer` interaction fails.
fn handle_no_videos_found_action(
    folder_path_display: &str, // Pass as &str to avoid cloning PathBuf just for display
    theme: &ColorfulTheme,     // Pass as &str for efficient display within the function.
    history: &[HistoryEntry],  // Pass history as a slice
    current_folder_path: &mut Option<PathBuf>,
    cached_folder_scan: &mut Option<(PathBuf, Vec<PathBuf>)>,
) -> Result<LoopControl, Box<dyn std::error::Error>> {
    println!("No video files found in '{}'.", folder_path_display);
    let action = Select::with_theme(theme)
        .with_prompt("No videos found. What would you like to do?")
        .items(&["Choose another folder", "View history", "Quit"])
        .default(0)
        .interact_opt()?
        .unwrap_or(2); // Default to Quit if Esc is pressed

    match action {
        0 => {
            // Choose another folder
            *current_folder_path = None;
            *cached_folder_scan = None;
            Ok(LoopControl::Continue)
        }
        1 => {
            // View history
            view_history(history, theme)?;
            // current_folder_path and cached_folder_scan remain unchanged.
            // The outer loop will re-evaluate.
            Ok(LoopControl::Continue)
        }
        _ => {
            // Quit
            Ok(LoopControl::Break)
        }
    }
}

/// Sets up and starts the Actix web server for streaming if not disabled.
///
/// # Arguments
///
/// * `no_streaming_flag` - Boolean indicating if streaming is explicitly disabled via CLI.
///
/// # Returns
///
/// `Ok(Some((ServerHandle, StreamState, String)))` containing the server handle,
/// shared stream state, and base URL if the server starts successfully.
/// `Ok(None)` if streaming is disabled (by flag, IP error, or server start error).
/// `Err` if an unexpected error occurs (though most errors are handled and result in `Ok(None)`).
async fn setup_streaming_server(
    no_streaming_flag: bool,
) -> Result<Option<(ServerHandle, StreamState, String)>, Box<dyn std::error::Error>> {
    if no_streaming_flag {
        println!("Streaming server is disabled via the --no-streaming flag.");
        return Ok(None);
    }

    let local_ip_addr = match local_ip() {
        Ok(ip) => ip.to_string(),
        Err(e) => {
            log::warn!(
                "Could not get local IP address for streaming: {}. Streaming will be disabled.",
                e
            );
            return Ok(None);
        }
    };

    if local_ip_addr.is_empty() {
        // Should ideally not happen if local_ip() succeeded
        log::error!(
            "Streaming disabled: local_ip() returned Ok, but the resulting IP address string was empty. This is unexpected."
        );
        return Ok(None);
    }

    let stream_url_base = format!("http://{}:{}", local_ip_addr, STREAMING_PORT);
    let stream_state_instance = Arc::new(Mutex::new(None::<PathBuf>));
    let app_state_for_server = web::Data::new(stream_state_instance.clone());

    match run_server(local_ip_addr.clone(), STREAMING_PORT, app_state_for_server) {
        Ok(server) => {
            let server_handle = server.handle();
            tokio::spawn(server); // Spawn the server to run in the background
            println!(
                "Streaming server is running at {}:{}",
                local_ip_addr, STREAMING_PORT
            );
            Ok(Some((
                server_handle,
                stream_state_instance,
                stream_url_base,
            )))
        }
        Err(e) => {
            log::error!(
                "Failed to start streaming server: {}. Streaming will be disabled.",
                e
            );
            Ok(None)
        }
    }
}

/// Main application logic.
///
/// This function orchestrates the entire application flow:
/// - Initializes environment, logger, and parses command-line arguments.
/// - Loads history and sets up the streaming server (if enabled).
/// - Enters a loop to prompt for folders, scan for videos, pick a video, and offer actions.
/// - Handles user interactions for playing, streaming, re-picking, changing folders, viewing history, and quitting.
async fn run_app() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    env_logger::init(); // Initialize logger for log::warn! and other log levels.
    let cli_args = Cli::parse();
    let theme = ColorfulTheme::default();
    let mut history: Vec<HistoryEntry> = load_history()?;

    // Setup streaming server
    let streaming_components = setup_streaming_server(cli_args.no_streaming).await?;

    // Destructure the components if streaming is enabled, otherwise they'll be None
    let (mut actix_server_main_handle, stream_state_arc, stream_url_base) =
        match streaming_components {
            Some((handle, state, url_base)) => (Some(handle), Some(state), Some(url_base)),
            None => (None, None, None),
        };

    let mut current_folder_path: Option<PathBuf> = cli_args
        .folder
        .map(|s| PathBuf::from(shellexpand::tilde(&s).into_owned()))
        .or_else(|| {
            env::var("DEFAULT_VIDEO_FOLDER")
                .ok()
                .map(|s| PathBuf::from(shellexpand::tilde(&s).into_owned()))
        });
    let scan_recursively = !cli_args.non_recursive;

    // Cache for video file paths: (folder_path, list_of_video_files)
    let mut cached_folder_scan: Option<(PathBuf, Vec<PathBuf>)> = None;

    'outer: loop {
        let folder_path_to_scan = match current_folder_path.clone() {
            Some(path) => path,
            None => {
                // Prompt for a new folder.
                // Invalidate the cache because we're about to get a new folder path,
                // so any previous scan results are for a different, now irrelevant, folder.
                cached_folder_scan = None;
                PathBuf::from(
                    shellexpand::full(
                        &Input::<String>::with_theme(&theme)
                            .with_prompt(
                                "Enter the path to the video folder (supports ~ and env vars)",
                            )
                            .interact_text()?,
                    )?
                    .into_owned(),
                )
            }
        };

        // Validate that the path is a directory before attempting to scan
        if !folder_path_to_scan.is_dir() {
            eprintln!(
                "The path '{}' is not a valid directory.",
                folder_path_to_scan.display()
            );
            current_folder_path = None; // Reset to re-prompt for folder
            continue 'outer; // Skip scanning and go back to the start of the loop
        } else {
            // Check if we can read the directory (implies read permissions)
            if let Err(e) = std::fs::read_dir(&folder_path_to_scan) {
                eprintln!(
                    "Error: Cannot access directory '{}'. Please check permissions. (Details: {})",
                    folder_path_to_scan.display(),
                    e
                );
                log::error!("Failed to read directory '{}': {}", folder_path_to_scan.display(), e);
                current_folder_path = None; // Reset to re-prompt for folder
                cached_folder_scan = None; // Clear cache as we couldn't access this folder
                continue 'outer;
            }
        }
        // Use cached scan if available and folder matches, otherwise scan
        let video_files_paths = match &cached_folder_scan {
            Some((cached_path, files)) if *cached_path == folder_path_to_scan => {
                println!("Using cached file list for '{}'.", cached_path.display());
                files.clone()
            }
            _ => {
                // Scan folder and update cache
                println!("Scanning folder '{}'...", folder_path_to_scan.display());
                match find_video_files(&folder_path_to_scan, scan_recursively) {
                    Ok(files) => {
                        cached_folder_scan = Some((folder_path_to_scan.clone(), files.clone()));
                        files
                    }
                    Err(e) => {
                        eprintln!(
                            "Error scanning folder '{}': {}",
                            folder_path_to_scan.display(),
                            e
                        );
                        current_folder_path = None; // Reset to re-prompt for folder
                        cached_folder_scan = None; // Clear cache on error
                        continue 'outer;
                    }
                }
            }
        };

        // Update current_folder_path to the one we just processed/scanned
        // This is important so that "Pick another from this folder" uses the correct path
        current_folder_path = Some(folder_path_to_scan.clone());

        if video_files_paths.is_empty() {
            match handle_no_videos_found_action(
                &folder_path_to_scan.to_string_lossy(), // Pass display string
                &theme,
                &history,
                &mut current_folder_path,
                &mut cached_folder_scan,
            )? {
                LoopControl::Continue => continue 'outer,
                LoopControl::Break => break 'outer,
            }
            // The continue 'outer or break 'outer handles the flow,
            // so no additional continue 'outer is needed here.
        }

        // Optimize pick_count calculation using a HashMap
        let history_pick_counts: HashMap<String, usize> = {
            let mut counts = HashMap::new();
            for entry in &history {
                *counts.entry(entry.path.clone()).or_insert(0) += 1;
            }
            counts
        };

        let video_entries: Vec<VideoEntry> = video_files_paths
            .iter() // Changed from into_iter() to keep video_files_paths available
            .map(|path_ref| {
                // path_ref is &PathBuf
                let path_str = path_ref.to_string_lossy();
                let pick_count = history_pick_counts
                    .get(path_str.as_ref())
                    .copied()
                    .unwrap_or(0);
                VideoEntry::new(path_ref.clone(), pick_count) // Clone path_ref here
            })
            .collect();

        // Verification: If video_files_paths was not empty (checked earlier),
        // video_entries should also not be empty after the mapping.
        // If it is empty, it's an unexpected state.
        if video_entries.is_empty() {
            // This implies that the initial video_files_paths list was not empty,
            // but the process of converting them to VideoEntry items resulted in an empty list.
            // This is unexpected with the current 1:1 mapping logic.
            log::error!(
                "Internal inconsistency: Found video files, but no video entries could be created. video_files_paths: {:?}. \
                This might indicate an issue with processing video file paths. The selection process will likely fail with 'No valid choice'.",
                video_files_paths
            );
            // No need to 'continue' or 'break' here; .choose_weighted() below will return an Err
            // if video_entries is empty, which will be propagated by the '?' operator.
        }

        let selected_video_entry = video_entries
            .choose_weighted(&mut rand::rng(), |item| item.weight())?
            .clone();
        let selected_file = selected_video_entry.path.clone();

        println!(
            "\n✨ Picked: {} (Previously picked {} times)",
            selected_file.display(),
            selected_video_entry.pick_count
        );

        if let Ok(metadata) = get_video_metadata(&selected_file) {
            println!(
                "Metadata: Resolution: {}, Duration: {}",
                metadata.resolution.unwrap_or_else(|| "N/A".into()),
                metadata.duration.unwrap_or_else(|| "N/A".into())
            );
        }

        add_to_history(&mut history, &selected_file)?;

        'inner: loop {
            let mut actions = vec!["Play locally"];
            let mut current_video_streaming_url: Option<String> = None;

            if let Some(state_arc_ref) = &stream_state_arc {
                if let Some(base_url_ref) = &stream_url_base {
                    let mut is_current_video_set_for_streaming = false;
                    if let Ok(guard) = state_arc_ref.lock() {
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

            actions.extend(vec![
                "Pick another from this folder",
                "Rescan current folder", // New action
                "Choose a different folder",
                "View history",
                "Quit",
            ]);

            let choice_prompt = format!(
                "Selected: '{}'. What next?",
                selected_file.file_name().map_or_else(
                    || selected_file.to_string_lossy(),
                    |name| name.to_string_lossy()
                )
            );

            let choice_idx = Select::with_theme(&theme)
                .with_prompt(&choice_prompt)
                .items(&actions)
                .default(0)
                .interact_opt()?
                .unwrap_or_else(|| {
                    actions
                        .iter()
                        .position(|a| *a == "Quit")
                        .unwrap_or(actions.len() - 1)
                });

            // Apply clippy suggestion: .map(|s| *s) -> .copied()
            match actions.get(choice_idx).copied() {
                Some("Play locally") => {
                    if let Err(e) = play_video_locally(&selected_file) {
                        eprintln!("Error playing video locally: {}", e);
                    }
                }
                Some("Stream this video") => {
                    if let (Some(state_arc_ref_update), Some(base_url_ref_display)) =
                        (&stream_state_arc, &stream_url_base)
                    {
                        {
                            let mut guard = state_arc_ref_update.lock().unwrap();
                            *guard = Some(selected_file.clone());
                        }
                        let full_stream_url = format!("{}/stream", base_url_ref_display);
                        println!(
                            "Streaming URL for '{}': {}",
                            selected_file.display(),
                            full_stream_url
                        );
                        if let Ok(code) = QrCode::new(full_stream_url.as_bytes()) {
                            println!(
                                "Scan QR code to stream on another device:\n{}",
                                code.render::<unicode::Dense1x2>().build()
                            );
                        }
                    } else {
                        println!("Streaming is not available or was not enabled for this session.");
                    }
                }
                Some("Get Streaming Link (current video)") => {
                    if let Some(url) = &current_video_streaming_url {
                        println!("Streaming URL for '{}': {}", selected_file.display(), url);
                        if let Ok(code) = QrCode::new(url.as_bytes()) {
                            println!(
                                "Scan QR code to stream on another device:\n{}",
                                code.render::<unicode::Dense1x2>().build()
                            );
                        }
                    } else {
                        println!("Streaming link is not available. Try 'Stream this video' first.");
                    }
                }
                Some("Pick another from this folder") => {
                    // current_folder_path is already set to the current folder.
                    // The cache will be used if it matches.
                    break 'inner;
                }
                Some("Rescan current folder") => {
                    if let Some(path_to_rescan) = &current_folder_path {
                        println!("Rescanning folder '{}'...", path_to_rescan.display());
                        // Invalidate cache for this specific folder to force a fresh scan
                        cached_folder_scan = None;
                        // current_folder_path remains the same.
                        // The next iteration of 'outer loop will call find_video_files
                        // because cached_folder_scan is None or its path won't match.
                        // Actually, we need to ensure the *next* scan is forced for *this* folder.
                        // Setting cached_folder_scan to None is enough, the outer loop will handle it.
                    } else { // This else block should ideally not be reached if the option is offered.
                        log::error!(
                            "Internal inconsistency: 'Rescan current folder' option was selected, \
                            but current_folder_path is None."
                        );
                        // This state should ideally not be reached if this option is offered.
                    }
                    break 'inner;
                }
                Some("Choose a different folder") => {
                    current_folder_path = None; // Clear folder to prompt for new one.
                    cached_folder_scan = None; // Clear cache as folder will change.
                    break 'inner;
                }
                Some("View history") => {
                    view_history(&history, &theme)?;
                }
                Some("Quit") | Some(_) | None => {
                    if let Some(server_handle_to_stop) = actix_server_main_handle.take() {
                        println!("\nStopping streaming server...");
                        server_handle_to_stop.stop(true).await;
                        println!("Streaming server stopped.");
                    }
                    println!("Goodbye!");
                    return Ok(());
                }
            }
        }
    }

    if let Some(server_handle_to_stop) = actix_server_main_handle.take() {
        server_handle_to_stop.stop(true).await;
    }
    Ok(())
}
