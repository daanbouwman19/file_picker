// src/main.rs

use actix_web::{dev::ServerHandle, web};
use clap::Parser;
use dialoguer::{theme::ColorfulTheme, Input, Select};
use local_ip_address::local_ip;
use qrcode::render::unicode;
use qrcode::QrCode;
use rand::prelude::*;
use std::collections::HashMap;
use std::env;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::{Arc, Mutex};

// Module declarations (ensure these match your project structure)
mod cli;
mod config;
mod file_utils;
mod history_manager;
mod metadata_retriever;
mod stream_server;
mod ui;
mod video_entry;

// Crate imports
use crate::cli::Cli;
use crate::file_utils::find_video_files;
use crate::history_manager::{add_to_history, load_history, HistoryEntry};
use crate::metadata_retriever::get_video_metadata;
use crate::stream_server::{run_server, StreamState}; // Assuming StreamState is pub
use crate::ui::view_history;
use crate::video_entry::VideoEntry;

const STREAMING_PORT: u16 = 8080;

/// Attempts to open a video file with the system's default media player.
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

/// Enum to control the flow of the main loop when no videos are found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopControl {
    Continue,
    Break,
}

/// Enum to control the flow of the outer loop after user actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PostActionOutcome {
    PickAnotherFromThisFolder,
    RescanCurrentFolder,
    ChooseDifferentFolder,
    QuitApplication,
}

// Custom application error type
#[derive(Debug)]
enum AppError {
    NoVideoEntriesAvailable,
    // Add other app-specific errors here as needed
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::NoVideoEntriesAvailable => {
                write!(f, "No video entries could be prepared for selection, even if video files were found.")
            }
        }
    }
}
impl std::error::Error for AppError {}

#[tokio::main]
async fn main() {
    if let Err(err) = run_app().await {
        eprintln!("\nApplication Error: {}", err);
        process::exit(1);
    }
}

/// Initializes application state: logger, CLI args, theme, and history.
fn initialize_app_state(
) -> Result<(Cli, ColorfulTheme, Vec<HistoryEntry>), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok(); // Load .env file if present
    env_logger::init(); // Initialize logger
    let cli_args = Cli::parse(); // Parse command line arguments
    let theme = ColorfulTheme::default(); // Set default theme for dialoguer
    let history = load_history()?; // Load historical data
    Ok((cli_args, theme, history))
}

/// Determines the initial folder path from CLI arguments or environment variables.
fn determine_initial_folder_path(cli_args: &Cli) -> Option<PathBuf> {
    cli_args
        .folder
        .as_ref()
        .map(|s| PathBuf::from(shellexpand::tilde(s).into_owned())) // Expand tilde for home dir
        .or_else(|| {
            env::var("DEFAULT_VIDEO_FOLDER") // Fallback to environment variable
                .ok()
                .map(|s| PathBuf::from(shellexpand::tilde(&s).into_owned()))
        })
}

/// Sets up and starts the Actix web server for streaming if not disabled.
async fn setup_streaming_server_logic(
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
                // Use log crate for warnings
                "Could not get local IP address for streaming: {}. Streaming will be disabled.",
                e
            );
            return Ok(None);
        }
    };

    if local_ip_addr.is_empty() {
        log::error!("Streaming disabled: local_ip() returned Ok, but IP address string was empty.");
        return Ok(None);
    }

    let stream_url_base = format!("http://{}:{}", local_ip_addr, STREAMING_PORT);
    let stream_state_instance = Arc::new(Mutex::new(None::<PathBuf>)); // State for current streaming file
    let app_state_for_server = web::Data::new(stream_state_instance.clone());

    match run_server(local_ip_addr.clone(), STREAMING_PORT, app_state_for_server) {
        Ok(server) => {
            let server_handle = server.handle();
            tokio::spawn(server); // Run server in a background task
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
            Ok(None) // Streaming disabled on error
        }
    }
}

/// Destructures the optional streaming components into individual options.
fn destructure_streaming_components(
    streaming_components: Option<(ServerHandle, StreamState, String)>,
) -> (Option<ServerHandle>, Option<StreamState>, Option<String>) {
    match streaming_components {
        Some((handle, state, url_base)) => (Some(handle), Some(state), Some(url_base)),
        None => (None, None, None), // All components are None if streaming is disabled
    }
}

/// Prompts the user for a folder path if one is not already set.
fn get_or_prompt_folder_path(
    current_folder_path_opt: &Option<PathBuf>, // Note: changed to &Option
    theme: &ColorfulTheme,
    cached_folder_scan: &mut Option<(PathBuf, Vec<PathBuf>)>, // Mutable to clear cache if prompting
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(path) = current_folder_path_opt {
        return Ok(path.clone());
    }

    // If no current path, prompt user and invalidate cache
    *cached_folder_scan = None;
    let input = Input::<String>::with_theme(theme)
        .with_prompt("Enter the path to the video folder (supports ~ and env vars)")
        .interact_text()?; // Propagates error if user cancels (e.g., Ctrl+C)
    Ok(PathBuf::from(shellexpand::full(&input)?.into_owned())) // Expand env vars and tilde
}

/// Validates if the given path is an accessible directory.
/// Updates `current_folder_path_opt` and `cached_folder_scan` to None on validation failure to force re-prompt.
fn validate_folder_path(
    folder_to_scan: &Path,
    current_folder_path_opt: &mut Option<PathBuf>,
    cached_folder_scan: &mut Option<(PathBuf, Vec<PathBuf>)>,
) -> bool {
    if !folder_to_scan.is_dir() {
        eprintln!(
            "The path '{}' is not a valid directory.",
            folder_to_scan.display()
        );
        *current_folder_path_opt = None; // Force re-prompt
        *cached_folder_scan = None;
        return false;
    }
    // Check read permissions
    if let Err(e) = std::fs::read_dir(folder_to_scan) {
        eprintln!(
            "Error: Cannot access directory '{}'. Please check permissions. (Details: {})",
            folder_to_scan.display(),
            e
        );
        log::error!(
            "Failed to read directory '{}': {}",
            folder_to_scan.display(),
            e
        );
        *current_folder_path_opt = None; // Force re-prompt
        *cached_folder_scan = None;
        return false;
    }
    true // Path is a valid, accessible directory
}

/// Scans the folder for video files, utilizing a cache.
/// Updates `current_folder_path_opt` and `cached_folder_scan` on error to force re-prompt.
fn scan_for_videos(
    folder_to_scan: &Path,
    scan_recursively: bool,
    cached_folder_scan: &mut Option<(PathBuf, Vec<PathBuf>)>,
    current_folder_path_opt: &mut Option<PathBuf>, // To reset on error
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    // Check cache first
    if let Some((cached_path, files)) = cached_folder_scan {
        if cached_path == folder_to_scan {
            println!("Using cached file list for '{}'.", cached_path.display());
            return Ok(files.clone());
        }
    }

    // If not in cache or different folder, scan
    println!("Scanning folder '{}'...", folder_to_scan.display());
    match find_video_files(folder_to_scan, scan_recursively) {
        Ok(files) => {
            *cached_folder_scan = Some((folder_to_scan.to_path_buf(), files.clone())); // Update cache
            Ok(files)
        }
        Err(e) => {
            eprintln!(
                "Error scanning folder '{}': {}",
                folder_to_scan.display(),
                e
            );
            *current_folder_path_opt = None; // Reset to re-prompt
            *cached_folder_scan = None; // Clear cache on error
            Err(e) // Propagate scan error
        }
    }
}

/// Handles user interaction when no video files are found in a directory.
fn handle_no_videos_found_action_logic(
    folder_path_display: &str,
    theme: &ColorfulTheme,
    history: &[HistoryEntry],
    current_folder_path: &mut Option<PathBuf>,
    cached_folder_scan: &mut Option<(PathBuf, Vec<PathBuf>)>,
) -> Result<LoopControl, Box<dyn std::error::Error>> {
    println!("No video files found in '{}'.", folder_path_display);
    let action = Select::with_theme(theme)
        .with_prompt("No videos found. What would you like to do?")
        .items(&["Choose another folder", "View history", "Quit"])
        .default(0)
        .interact_opt()? // Returns Option<usize>, None if Esc
        .unwrap_or(2); // Default to Quit (index 2) if Esc is pressed

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
            Ok(LoopControl::Continue) // Continue outer loop, re-evaluating current folder
        }
        _ => {
            // Quit
            Ok(LoopControl::Break)
        }
    }
}

/// Selects a video from the list using weighted random choice based on pick history.
fn select_video_logic(
    video_files_paths: &[PathBuf],
    history: &[HistoryEntry],
) -> Result<VideoEntry, Box<dyn std::error::Error>> {
    // Calculate pick counts from history
    let history_pick_counts: HashMap<String, usize> = {
        let mut counts = HashMap::new();
        for entry in history {
            *counts.entry(entry.path.clone()).or_insert(0) += 1;
        }
        counts
    };

    // Create VideoEntry objects with pick counts
    let video_entries: Vec<VideoEntry> = video_files_paths
        .iter()
        .map(|path_ref| {
            let path_str = path_ref.to_string_lossy();
            let pick_count = history_pick_counts
                .get(path_str.as_ref())
                .copied()
                .unwrap_or(0);
            VideoEntry::new(path_ref.clone(), pick_count)
        })
        .collect();

    // This check is crucial. If video_files_paths was non-empty but video_entries is empty,
    // it indicates an issue in the mapping logic (though unlikely with current code).
    if video_entries.is_empty() {
        if video_files_paths.is_empty() {
            log::debug!("select_video_logic called with no video_files_paths, thus no entries.");
        } else {
            log::error!(
                "Internal inconsistency: Found video files ({}), but no video entries created. Paths: {:?}",
                video_files_paths.len(), video_files_paths
            );
        }
        // Return an error that signifies no items are available for selection.
        // This could be a custom error or a standard one like NotFound.
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            AppError::NoVideoEntriesAvailable.to_string(),
        ))); // Return our custom error, boxed
    }

    // Perform weighted random selection
    video_entries
        .choose_weighted(&mut rand::rng(), |item| item.weight())
        .cloned()
        .map_err(|e| {
            log::error!(
                "Error during weighted choice: {:?}. Video entries count: {}",
                e,
                video_entries.len()
            );
            Box::new(e) as Box<dyn std::error::Error> // Convert WeightedError
        })
}

/// Displays information about the selected video (path, pick count, metadata).
fn display_selected_video_info(selected_video_entry: &VideoEntry) {
    println!(
        "\n✨ Picked: {} (Previously picked {} times)",
        selected_video_entry.path.display(),
        selected_video_entry.pick_count
    );
    // Attempt to get and display video metadata
    if let Ok(metadata) = get_video_metadata(&selected_video_entry.path) {
        println!(
            "Metadata: Resolution: {}, Duration: {}",
            metadata.resolution.unwrap_or_else(|| "N/A".into()),
            metadata.duration.unwrap_or_else(|| "N/A".into())
        );
    } else {
        println!("Metadata: Could not retrieve metadata for this video.");
    }
}

/// Handles the inner loop of user actions for a selected video.
async fn loop_user_actions(
    selected_video_entry: &VideoEntry,
    history: &[HistoryEntry], // Pass as slice, history is updated before this call
    theme: &ColorfulTheme,
    stream_state_arc: &Option<StreamState>,
    stream_url_base: &Option<String>,
) -> Result<PostActionOutcome, Box<dyn std::error::Error>> {
    let selected_file = &selected_video_entry.path;

    loop {
        let mut actions = vec!["Play locally"];
        let mut current_video_streaming_url: Option<String> = None;

        // Dynamically add streaming-related actions
        if let (Some(state_arc_ref), Some(base_url_ref)) = (stream_state_arc, stream_url_base) {
            let mut is_current_video_set_for_streaming = false;
            // Check if the current selected video is already set for streaming
            if let Ok(guard) = state_arc_ref.lock() {
                // Lock the mutex to access stream state
                if let Some(streaming_path) = &*guard {
                    if streaming_path == selected_file {
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

        actions.extend(vec![
            "Pick another from this folder",
            "Rescan current folder",
            "Choose a different folder",
            "View history",
            "Quit",
        ]);

        let choice_prompt = format!(
            "Selected: '{}'. What next?",
            selected_file.file_name().map_or_else(
                // Display filename or full path
                || selected_file.to_string_lossy(),
                |name| name.to_string_lossy()
            )
        );

        // Prompt user for action
        let choice_idx = Select::with_theme(theme)
            .with_prompt(&choice_prompt)
            .items(&actions)
            .default(0) // Default to "Play locally"
            .interact_opt()? // None if Esc
            .unwrap_or_else(|| {
                // Default to "Quit" if Esc
                actions
                    .iter()
                    .position(|a| *a == "Quit")
                    .unwrap_or(actions.len() - 1)
            });

        // Match the chosen action
        match actions.get(choice_idx).copied() {
            Some("Play locally") => {
                if let Err(e) = play_video_locally(selected_file) {
                    eprintln!("Error playing video locally: {}", e);
                }
                // Continue inner loop for more actions on the same video
            }
            Some("Stream this video") => {
                if let (Some(state_arc_ref_update), Some(base_url_ref_display)) =
                    (stream_state_arc, stream_url_base)
                {
                    // Update the shared state with the new video path for streaming
                    {
                        // Scope for MutexGuard
                        let mut guard = state_arc_ref_update.lock().unwrap();
                        *guard = Some(selected_file.clone());
                    }
                    let full_stream_url = format!("{}/stream", base_url_ref_display);
                    println!(
                        "Streaming URL for '{}': {}",
                        selected_file.display(),
                        full_stream_url
                    );
                    // Display QR code for the streaming URL
                    if let Ok(code) = QrCode::new(full_stream_url.as_bytes()) {
                        println!(
                            "Scan QR code to stream on another device:\n{}",
                            code.render::<unicode::Dense1x2>().build()
                        );
                    }
                } else {
                    println!("Streaming is not available or was not enabled for this session.");
                }
                // Continue inner loop
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
                    // This case should ideally not be hit if logic for adding actions is correct
                    println!("Streaming link is not available. Try 'Stream this video' first.");
                }
                // Continue inner loop
            }
            Some("Pick another from this folder") => {
                return Ok(PostActionOutcome::PickAnotherFromThisFolder)
            }
            Some("Rescan current folder") => return Ok(PostActionOutcome::RescanCurrentFolder),
            Some("Choose a different folder") => {
                return Ok(PostActionOutcome::ChooseDifferentFolder)
            }
            Some("View history") => {
                view_history(history, theme)?; // Display history
                                               // Continue inner loop
            }
            Some("Quit") | Some(_) | None => return Ok(PostActionOutcome::QuitApplication), // Quit or any other unhandled
        }
    } // End of 'inner loop
}

/// Stops the streaming server if it's running.
/// Includes a timeout to prevent the application from hanging.
async fn shutdown_streaming_server_logic(server_handle: ServerHandle) {
    println!("\nStopping streaming server...");
    // Graceful stop with a timeout
    match tokio::time::timeout(std::time::Duration::from_secs(10), server_handle.stop(true)).await {
        Ok(_) => println!("Streaming server stopped."),
        Err(_) => eprintln!("Streaming server stop timed out!"),
    }
}

/// Main application logic, orchestrating the video picking process.
async fn run_app() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Initialization
    let (cli_args, theme, mut history) = initialize_app_state()?;

    // 2. Setup Streaming Server
    let streaming_components_opt = setup_streaming_server_logic(cli_args.no_streaming).await?;
    let (mut actix_server_main_handle, stream_state_arc, stream_url_base) =
        destructure_streaming_components(streaming_components_opt);

    // 3. Initial Folder Path & Scan Configuration
    let mut current_folder_path_opt: Option<PathBuf> = determine_initial_folder_path(&cli_args);
    let scan_recursively = !cli_args.non_recursive;
    let mut cached_folder_scan: Option<(PathBuf, Vec<PathBuf>)> = None;

    // 4. Main Application Loop
    'outer: loop {
        // 4.1. Determine Folder to Scan (Prompt if necessary)
        let folder_to_scan = match get_or_prompt_folder_path(
            &current_folder_path_opt, // Pass as immutable ref
            &theme,
            &mut cached_folder_scan,
        ) {
            Ok(path) => path,
            Err(e) => {
                // Error during folder prompt (e.g., user cancellation)
                log::info!(
                    "Exiting due to error or cancellation in folder prompt: {}",
                    e
                );
                break 'outer;
            }
        };

        // 4.2. Validate Folder Path
        if !validate_folder_path(
            &folder_to_scan,
            &mut current_folder_path_opt,
            &mut cached_folder_scan,
        ) {
            continue 'outer; // Validation failed, current_folder_path_opt is now None, will re-prompt
        }
        // If validation passed and we got here via prompt, current_folder_path_opt might still be None.
        // Set it to the successfully validated folder_to_scan.
        current_folder_path_opt = Some(folder_to_scan.clone());

        // 4.3. Scan for Video Files (with caching)
        let video_files_paths = match scan_for_videos(
            &folder_to_scan, // This is now guaranteed to be a valid directory path
            scan_recursively,
            &mut cached_folder_scan,
            &mut current_folder_path_opt, // Pass mutably to allow reset on scan error
        ) {
            Ok(paths) => paths,
            Err(_) => continue 'outer, // Error during scan, current_folder_path_opt reset, will re-prompt
        };

        // At this point, current_folder_path_opt should reflect folder_to_scan
        // as scan_for_videos would have used it or it was set before.
        // Ensure it's updated for "Pick another from this folder" to work correctly.
        // This was already handled by validate_folder_path and the logic in get_or_prompt_folder_path.

        // 4.4. Handle No Videos Found
        if video_files_paths.is_empty() {
            match handle_no_videos_found_action_logic(
                &folder_to_scan.to_string_lossy(),
                &theme,
                &history, // Pass immutable slice
                &mut current_folder_path_opt,
                &mut cached_folder_scan,
            )? {
                LoopControl::Continue => continue 'outer,
                LoopControl::Break => break 'outer,
            }
        }

        // 4.5. Select a Video
        let selected_video_entry = match select_video_logic(&video_files_paths, &history) {
            Ok(entry) => entry,
            Err(e) => {
                log::error!(
                    "Failed to select a video from '{}': {}. Video files found: {}.",
                    folder_to_scan.display(),
                    e,
                    video_files_paths.len()
                );
                eprintln!("Could not select a video: {}", e);
                // To recover, try prompting for a folder again.
                current_folder_path_opt = None;
                cached_folder_scan = None;
                continue 'outer;
            }
        };

        display_selected_video_info(&selected_video_entry);
        add_to_history(&mut history, &selected_video_entry.path)?; // Update history

        // 4.6. Handle User Actions for the Selected Video (Inner Loop)
        let action_outcome = loop_user_actions(
            &selected_video_entry,
            &history, // Pass immutable slice of updated history
            &theme,
            &stream_state_arc,
            &stream_url_base,
        )
        .await?;

        // 4.7. Process Outcome of Inner Loop
        match action_outcome {
            PostActionOutcome::PickAnotherFromThisFolder => {
                // current_folder_path_opt is already set to the current folder.
                // Cache will be used if still valid for this folder.
                continue 'outer;
            }
            PostActionOutcome::RescanCurrentFolder => {
                // current_folder_path_opt should still be the current folder.
                // Invalidate cache for this specific folder to force a fresh scan.
                if current_folder_path_opt.is_some() {
                    cached_folder_scan = None;
                } else {
                    // This state should ideally not be reached if "Rescan" was an option.
                    log::error!("'Rescan current folder' chosen, but no current folder path is set. Prompting for new folder.");
                    current_folder_path_opt = None; // Force re-prompt
                    cached_folder_scan = None;
                }
                continue 'outer;
            }
            PostActionOutcome::ChooseDifferentFolder => {
                current_folder_path_opt = None; // Clear current folder to trigger prompt
                cached_folder_scan = None; // Clear cache as folder is changing
                continue 'outer;
            }
            PostActionOutcome::QuitApplication => {
                break 'outer; // Exit the main application loop
            }
        }
    } // End of 'outer loop

    // 5. Shutdown Streaming Server (if it was started)
    if let Some(server_handle) = actix_server_main_handle.take() {
        // .take() to consume the Option
        shutdown_streaming_server_logic(server_handle).await;
    }
    println!("Goodbye!");
    Ok(())
}
