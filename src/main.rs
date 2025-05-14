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

/// Attempts to open the given video file path with the system's default application.
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

#[tokio::main]
async fn main() {
    if let Err(err) = run_app().await {
        eprintln!("\nApplication Error: {}", err);
        process::exit(1);
    }
}

async fn run_app() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    let cli_args = Cli::parse();
    let theme = ColorfulTheme::default();
    let mut history: Vec<HistoryEntry> = load_history()?;

    let mut actix_server_main_handle: Option<ServerHandle> = None;
    let mut stream_state_arc: Option<StreamState> = None;
    let mut stream_url_base: Option<String> = None;

    if !cli_args.no_streaming {
        let local_ip_addr = match local_ip() {
            Ok(ip) => ip.to_string(),
            Err(e) => {
                eprintln!(
                    "Warning: Could not get local IP address for streaming: {}. Streaming will be disabled.",
                    e
                );
                String::new()
            }
        };

        if !local_ip_addr.is_empty() {
            stream_url_base = Some(format!("http://{}:{}", local_ip_addr, STREAMING_PORT));
            let state_for_server_instance = Arc::new(Mutex::new(None::<PathBuf>));
            stream_state_arc = Some(state_for_server_instance.clone());
            let app_state_for_server_config = web::Data::new(state_for_server_instance);

            match run_server(
                local_ip_addr.clone(),
                STREAMING_PORT,
                app_state_for_server_config,
            ) {
                Ok(server) => {
                    actix_server_main_handle = Some(server.handle());
                    tokio::spawn(server);
                    println!(
                        "Streaming server is running at {}:{}",
                        local_ip_addr, STREAMING_PORT
                    );
                }
                Err(e) => {
                    eprintln!(
                        "Failed to start streaming server: {}. Streaming will be disabled.",
                        e
                    );
                    stream_state_arc = None;
                    stream_url_base = None;
                }
            }
        } else if !cli_args.no_streaming {
            println!("Streaming disabled: Could not determine local IP address.");
        }
    } else {
        println!("Streaming server is disabled via the --no-streaming flag.");
    }

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
                // Prompt for new folder, invalidate cache
                cached_folder_scan = None;
                PathBuf::from(
                    shellexpand::full(
                        &Input::<String>::with_theme(&theme)
                            .with_prompt("Enter the path to the video folder (supports ~ and env vars)")
                            .interact_text()?,
                    )?
                    .into_owned(),
                )
            }
        };

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
                        eprintln!("Error scanning folder '{}': {}", folder_path_to_scan.display(), e);
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
            println!("No video files found in '{}'.", folder_path_to_scan.display());
            let action = Select::with_theme(&theme)
                .with_prompt("No videos found. What would you like to do?")
                .items(&["Choose another folder", "View history", "Quit"])
                .default(0)
                .interact_opt()?
                .unwrap_or(2);

            match action {
                0 => {
                    current_folder_path = None; // Prompt for a new folder.
                    cached_folder_scan = None; // Clear cache as folder will change.
                }
                1 => {
                    view_history(&history, &theme)?;
                    // Don't change current_folder_path or cache here,
                    // loop will re-evaluate based on existing current_folder_path
                }
                _ => break 'outer, 
            }
            continue 'outer; 
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
            .into_iter()
            .map(|path| {
                let path_str = path.to_string_lossy();
                let pick_count = history_pick_counts.get(path_str.as_ref()).copied().unwrap_or(0);
                VideoEntry::new(path, pick_count)
            })
            .collect();

        if video_entries.is_empty() { // Should be caught by video_files_paths.is_empty(), but as a safeguard
            println!("No video entries could be created (this shouldn't happen if files were found).");
            current_folder_path = None;
            cached_folder_scan = None;
            continue 'outer; 
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
                    } else {
                        println!("No current folder to rescan.");
                        // This state should ideally not be reached if this option is offered.
                    }
                    break 'inner; 
                }
                Some("Choose a different folder") => {
                    current_folder_path = None; // Clear folder to prompt for new one.
                    cached_folder_scan = None;  // Clear cache as folder will change.
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
