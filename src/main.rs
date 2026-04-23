use anyhow::{bail, Context, Result};

#[cfg(windows)]
mod mcp_client;
mod profiles;
#[cfg(windows)]
mod stream_deck_app;

use clap::{Parser, Subcommand};

const ABOUT: &str = "\
Elgato Stream Deck utilities.\n\
";

#[derive(Parser)]
#[command(name = "streamdeck_tools", version, about = ABOUT, propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Lists all profile display names, profile device models, and the profile ids
    ListProfiles,
    /// Adds actions to the MCP Actions profile to swap between all of your profiles
    AddProfileActions,
    /// Lists all the mcp action's names, titles, and ids
    ListActions,
    /// Run one of the actions form the list-actions command
    RunAction {
        /// ActionID from list-actions.
        action_id: String,
    },
}

#[cfg(windows)]
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::ListProfiles => cmd_list_profiles(),
        Commands::AddProfileActions => cmd_add_profile_actions(),
        Commands::ListActions => cmd_list_actions(),
        Commands::RunAction { action_id } => cmd_run_action(&action_id).await,
    }
}

#[cfg(windows)]
fn cmd_list_profiles() -> Result<()> {
    let list = profiles::get_profiles()?.into_iter()
        .filter(|p| p.device_model != profiles::AI_STREAM_DECK_MODEL);
    for p in list {
        println!("{}\t{}\t{}", p.name, p.device_model, p.id);
    }
    Ok(())
}

fn verify_mcp_enabled() -> Result<()> {
    if !mcp_client::McpSession::can_connect() {
        bail!("Make sure 'MCP Actions' is enabled in your Stream Deck settings")
    }
    Ok(())
}

#[cfg(windows)]
fn cmd_add_profile_actions() -> Result<()> {
    verify_mcp_enabled()?;

    let profiles_dir = profiles::get_profiles_dir()?;
    let stream_deck_exe = stream_deck_app::stream_deck_exe_from_running_processes();
    eprintln!("Stopping Stream Deck process");
    stream_deck_app::stop_stream_deck()?;

    let write_result = (|| {
        let (ai_profile_dir, ai_profile_json) = profiles::find_ai_stream_deck_profile(&profiles_dir)?;
        let (added, skipped, reasons) =
            profiles::add_profile_switch_actions(&profiles_dir, &ai_profile_dir, &ai_profile_json)?;
        Ok::<_, anyhow::Error>((added, skipped, reasons))
    })();

    eprintln!("Starting Stream Deck");
    if let Some(exe) = stream_deck_exe {
        if let Err(e) = stream_deck_app::start_stream_deck(&exe) {
            eprintln!("ERROR: could not restart Stream Deck: {e:#}");
        }
    }

    let (added, skipped, reasons) = write_result.context("add profile switch actions")?;
    println!("Switch Profile actions added: {added}");
    if !skipped.is_empty() {
        eprintln!("Skipped {} profile(s) (not enough empty slots or missing manifest):", skipped.len());
        for (id, reason) in skipped.iter().zip(reasons.iter()) {
            eprintln!("  {id} — {reason}");
        }
    }
    Ok(())
}

#[cfg(windows)]
fn cmd_list_actions() -> Result<()> {
    verify_mcp_enabled()?;

    let root = profiles::get_profiles_dir()?;
    let (profile_dir, _) = profiles::find_ai_stream_deck_profile(&root)?;
    let actions = profiles::iter_ai_profile_actions(&profile_dir)?;
    for action in actions {
        let name = action.name;
        let title = action.title;
        let action_id = action.id;
        println!(
            "{name}\t{title}\t{action_id}"
        );
    }
    Ok(())
}

#[cfg(windows)]
async fn cmd_run_action(action_id: &str) -> Result<()> {
    verify_mcp_enabled()?;
    
    let mut session = mcp_client::McpSession::connect()
        .await
        .with_context(|| format!("connect to {}", mcp_client::PIPE_NAME))?;
    
    let tools_resp = session.tools_list().await?;
    let tool_name = mcp_client::resolve_run_action_tool(&tools_resp)?;
    session.call_tool(&tool_name, serde_json::json!({ "id": action_id })).await?;
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Only Windows is supported atm");
    std::process::exit(1);
}
