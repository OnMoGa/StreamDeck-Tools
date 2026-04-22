//! Stop / start the Elgato Stream Deck desktop app on Windows.

use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use sysinfo::{ProcessesToUpdate, System};

const SD_PROCESS_NAME: &str = "StreamDeck.exe";

/// If a Stream Deck process is running, return the path to its executable
pub fn stream_deck_exe_from_running_processes() -> Option<PathBuf> {
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);

    for process in sys.processes().values() {
        if process.name() != SD_PROCESS_NAME {
            continue;
        }
        let Some(exe) = process.exe() else {
            continue;
        };
        if exe.as_os_str().is_empty() {
            continue;
        }
        return Some(exe.to_path_buf());
    }
    None
}

pub fn stop_stream_deck() -> Result<()> {
    Command::new("taskkill")
        .args(["/IM", SD_PROCESS_NAME, "/F", "/T"])
        .output()
        .with_context(|| "Failed to terminate the Stream Deck process")?;
    
    thread::sleep(Duration::from_millis(800));
    Ok(())
}


pub fn start_stream_deck(preferred_exe: &Path) -> Result<()> {
    if preferred_exe.is_file()
    {
        Command::new(preferred_exe)
            .spawn()
            .with_context(|| format!("failed to spawn {}", preferred_exe.display()))?;
        return Ok(());
    }
    bail!("Failed to relaunch the Stream Deck process.");
}
