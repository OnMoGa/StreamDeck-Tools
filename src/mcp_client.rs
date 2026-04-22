//! JSON-RPC over newline-delimited messages on the Elgato MCP named pipe.

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::ClientOptions;
use tokio::sync::{oneshot, Mutex};

pub const PIPE_NAME: &str = r"\\.\pipe\elgato-mcp-streamdeck";

type PendingMap = Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolRequest<'a> {
    pub id: String,
    pub method: &'static str,
    pub tool_name: &'a str,
    pub arguments: Value,
}

async fn read_loop(
    mut reader: BufReader<tokio::io::ReadHalf<tokio::net::windows::named_pipe::NamedPipeClient>>,
    pending: PendingMap,
) -> Result<()> {
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Err(anyhow!("IPC connection closed"));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let msg: Value = serde_json::from_str(trimmed)
            .with_context(|| format!("invalid JSON from Stream Deck: {trimmed}"))?;

        let method = msg.get("method").and_then(|m| m.as_str());
        let id = msg.get("id").and_then(|i| i.as_str());

        match (id, method) {
            (Some(id), Some("elicitation/create")) => {
                eprintln!("(ignored) elicitation/create id={id}");
            }
            (Some(id), None) => {
                let mut guard = pending.lock().await;
                if let Some(tx) = guard.remove(id) {
                    drop(guard);
                    let _ = tx.send(msg);
                }
            }
            (None, Some(m)) => {
                eprintln!("(info) notification method={m}");
            }
            _ => {
                eprintln!("(info) unrecognized line: {trimmed}");
            }
        }
    }
}

async fn send_request(
    write: &mut tokio::io::WriteHalf<tokio::net::windows::named_pipe::NamedPipeClient>,
    pending: &PendingMap,
    body: Value,
) -> Result<Value> {
    let id = body
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("request missing id"))?
        .to_owned();

    let (tx, rx) = oneshot::channel();
    pending.lock().await.insert(id.clone(), tx);

    let mut payload = serde_json::to_string(&body)?;
    payload.push('\n');
    write.write_all(payload.as_bytes()).await?;
    write.flush().await?;

    rx.await
        .map_err(|_| anyhow!("IPC reader dropped before response for id={id}"))
}

pub struct McpSession {
    write: tokio::io::WriteHalf<tokio::net::windows::named_pipe::NamedPipeClient>,
    pending: PendingMap,
    read_task: tokio::task::JoinHandle<Result<()>>,
}

impl McpSession {
    pub async fn connect() -> Result<Self> {
        let client = ClientOptions::new()
            .open(PIPE_NAME)
            .with_context(|| format!("open named pipe {PIPE_NAME}"))?;

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let (read_half, write_half) = tokio::io::split(client);
        let reader = BufReader::new(read_half);
        let pending_reader = Arc::clone(&pending);

        let read_task = tokio::spawn(async move { read_loop(reader, pending_reader).await });

        Ok(Self {
            write: write_half,
            pending,
            read_task,
        })
    }

    pub async fn request(&mut self, body: Value) -> Result<Value> {
        send_request(&mut self.write, &self.pending, body).await
    }

    pub async fn tools_list(&mut self) -> Result<Value> {
        let req = json!({
            "id": uuid::Uuid::new_v4().to_string(),
            "method": "tools_list",
        });
        self.request(req).await.context("tools_list")
    }

    pub async fn call_tool(&mut self, tool_name: &str, arguments: Value) -> Result<Value> {
        let call = CallToolRequest {
            id: uuid::Uuid::new_v4().to_string(),
            method: "call_tool",
            tool_name,
            arguments,
        };
        let call_val = serde_json::to_value(&call)?;
        self.request(call_val)
            .await
            .with_context(|| format!("call_tool {tool_name}"))
    }

    pub fn abort_reader(self) {
        self.read_task.abort();
    }
}

/// Pick the Elgato tool used to run an action by id.
pub fn resolve_run_action_tool(tools_response: &Value) -> Result<String> {
    let tools = tools_response
        .pointer("/result/tools")
        .or_else(|| tools_response.get("tools"))
        .and_then(|t| t.as_array())
        .ok_or_else(|| anyhow!("tools_list response missing result.tools array"))?;

    let mut matches: Vec<&str> = Vec::new();
    for t in tools {
        let Some(name) = t.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        let looks_like_run = (lower.contains("run") || lower.contains("trigger") || lower.contains("execute"))
            && lower.contains("action");
        if looks_like_run {
            matches.push(name);
        }
    }

    if matches.is_empty() {
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
            .collect();
        bail!(
            "No tool matching 'run' + 'action' in name. Available tools: {}",
            names.join(", ")
        );
    }
    if matches.len() > 1 {
        bail!(
            "Multiple run-action tools matched: {}. Specify Elgato tool naming in code.",
            matches.join(", ")
        );
    }

    Ok(matches[0].to_string())
}
