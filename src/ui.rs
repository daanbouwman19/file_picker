// src/ui.rs

use crate::history_manager::HistoryEntry;
use chrono::{DateTime, Local}; // Use Local timezone for display purposes.
use dialoguer::{theme::ColorfulTheme, Input, Select};
use std::path::PathBuf;

/// Displays recent video history entries in an interactive list.
/// Shows up to the 20 most recent entries.
/// Allows the user to select an entry to view its full path and timestamp.
///
/// # Arguments
///
/// * `history` - A slice of `HistoryEntry` items, assumed to be sorted newest first.
/// * `theme` - The `dialoguer::theme::ColorfulTheme` to use for prompts.
///
/// # Errors
///
/// Returns an error if any dialoguer interaction fails.
pub fn view_history(
    history: &[HistoryEntry],
    theme: &ColorfulTheme,
) -> Result<(), Box<dyn std::error::Error>> {
    if history.is_empty() {
        println!("\n--- History is empty ---");
        Input::<String>::with_theme(theme)
            .with_prompt("Press Enter to continue...")
            .allow_empty(true)
            .interact()?;
        return Ok(());
    }

    // Prepare items for the selection list, limiting to the 20 most recent.
    let items: Vec<String> = history
        .iter()
        .take(20)
        .map(|entry| {
            let local_time: DateTime<Local> = DateTime::from(entry.picked_at); // Convert UTC to local time for display.
            let file_name = PathBuf::from(&entry.path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| entry.path.clone()); // Fallback to full path if filename cannot be extracted.
            format!(
                "{} (picked on {})",
                file_name,
                local_time.format("%Y-%m-%d %H:%M") // User-friendly date/time format.
            )
        })
        .collect();

    // Optional: Attempt to clear the screen. This might fail on some terminals.
    // if let Err(e) = dialoguer::console::Term::stdout().clear_screen() {
    //     eprintln!("Note: Failed to clear screen before history view: {}", e);
    // }

    let selection = Select::with_theme(theme)
        .with_prompt("-- Video History (Recent first, max 20) --\nSelect to view full path, Esc to go back:")
        .items(&items)
        .default(0) // Default to the most recent entry.
        .interact_opt()?; // Returns Option<usize>; None if Esc is pressed.

    if let Some(index) = selection {
        if let Some(selected_entry) = history.get(index) {
            // Optional: Clear screen again before showing details.
            // if let Err(e) = dialoguer::console::Term::stdout().clear_screen() {
            //      eprintln!("Note: Failed to clear screen before showing details: {}", e);
            // }

            println!("\n--- Selected History Entry ---");
            println!("Full path: {}", selected_entry.path);
            println!(
                "Picked at: {}",
                DateTime::<Local>::from(selected_entry.picked_at).format("%Y-%m-%d %H:%M:%S %Z")
            );
            println!("------------------------------");

            Input::<String>::with_theme(theme)
                .with_prompt("Press Enter to continue...")
                .allow_empty(true)
                .interact()?;
        }
    }
    // If selection is None (user pressed Esc), simply return.

    Ok(())
}