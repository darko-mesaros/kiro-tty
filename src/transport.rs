//! Process + JSON-RPC transport for talking to `kiro-cli acp`.
//!
//! Responsibilities:
//!   * spawn `kiro-cli acp` with piped stdio and keep it alive,
//!   * serialize outbound JSON-RPC requests / notifications to the child stdin,
//!   * run a reader task that turns each newline-delimited stdout record into an
//!     ordered [`AcpEvent`] on an mpsc channel,
//!   * drain the child stderr to an optional debug log so it never pollutes the
//!     terminal or blocks the child by filling the pipe buffer.
//!
//! Classification is the crux. Kiro interleaves responses with a flood of
//! `_kiro.dev/*` extension notifications, so the reader can never assume "the
//! next line is my response". Every line is inspected and routed by shape.

use std::process::Stdio;

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

use crate::protocol::{SessionNotification, SessionUpdate};

/// An ordered event emitted by the reader task. Order matches the order lines
/// arrived on the child's stdout, which is what lets the main loop treat a
/// prompt turn as "render updates until my response arrives".
#[derive(Debug)]
pub enum AcpEvent {
    /// A response to one of our requests, keyed by the id we assigned.
    Response { id: u64, result: RpcResult },
    /// A `session/update` notification we care about.
    Update(SessionUpdate),
    /// A server -> client request (has both `method` and `id`). We don't expect
    /// these in trust-all demo mode, but we surface them so the main loop can
    /// answer rather than let the agent hang.
    ServerRequest { id: Value, method: String },
    /// The child closed its stdout (EOF) — the agent is gone.
    Closed,
}

/// A JSON-RPC result: either the `result` payload or an `error` object.
#[derive(Debug)]
pub enum RpcResult {
    Ok(Value),
    Err(Value),
}

pub struct Transport {
    child: Child,
    stdin: ChildStdin,
    next_id: u64,
    events: mpsc::Receiver<AcpEvent>,
}

impl Transport {
    /// Spawn `kiro-cli acp` and wire up the reader/stderr tasks.
    ///
    /// `extra_args` are appended after `acp` (e.g. `--trust-all-tools`,
    /// `--agent`, `--model`). `log_path`, if set, receives the child's stderr.
    pub fn spawn(extra_args: &[String], log_path: Option<&str>) -> Result<Self> {
        let mut cmd = Command::new("kiro-cli");
        cmd.arg("acp")
            .args(extra_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Belt-and-suspenders against the agent emitting color even though
            // ACP output is already clean JSON.
            .env("NO_COLOR", "1")
            .env("KIRO_LOG_NO_COLOR", "1")
            .kill_on_drop(true);

        let mut child = cmd.spawn().context("failed to spawn `kiro-cli acp`")?;

        let stdin = child.stdin.take().context("child stdin was not piped")?;
        let stdout = child.stdout.take().context("child stdout was not piped")?;
        let stderr = child.stderr.take().context("child stderr was not piped")?;

        // Channel is bounded: it provides backpressure so a burst of Kiro
        // notifications can't grow memory without limit.
        let (tx, rx) = mpsc::channel::<AcpEvent>(256);

        // Reader task: classify every stdout line into an AcpEvent.
        tokio::spawn(reader_loop(stdout, tx));

        // stderr drain: to a file if requested, otherwise discarded. Either way
        // we must read it so the pipe buffer never fills and stalls the child.
        tokio::spawn(drain_stderr(stderr, log_path.map(|s| s.to_string())));

        Ok(Self {
            child,
            stdin,
            next_id: 0,
            events: rx,
        })
    }

    /// Send a JSON-RPC request and return the id assigned to it.
    pub async fn request<P: Serialize>(&mut self, method: &str, params: P) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write_line(&msg).await?;
        Ok(id)
    }

    /// Send a JSON-RPC notification (no id, no response expected).
    pub async fn notify<P: Serialize>(&mut self, method: &str, params: P) -> Result<()> {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_line(&msg).await
    }

    /// Reply to a server -> client request with a JSON-RPC error. Used to
    /// politely decline anything we don't implement so the agent doesn't hang.
    pub async fn respond_error(&mut self, id: Value, code: i64, message: &str) -> Result<()> {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message },
        });
        self.write_line(&msg).await
    }

    async fn write_line(&mut self, msg: &Value) -> Result<()> {
        let mut line = serde_json::to_vec(msg).context("failed to serialize JSON-RPC message")?;
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .await
            .context("failed to write to child stdin")?;
        self.stdin.flush().await.context("failed to flush child stdin")?;
        Ok(())
    }

    /// Await the next transport event.
    pub async fn next_event(&mut self) -> AcpEvent {
        self.events.recv().await.unwrap_or(AcpEvent::Closed)
    }

    /// Best-effort shutdown: close stdin (signals EOF to the agent) and kill the
    /// child if it doesn't exit promptly.
    pub async fn shutdown(mut self) {
        // Dropping stdin closes it; do it explicitly for clarity.
        let _ = self.stdin.shutdown().await;
        // Give the child a moment to exit on its own, then kill.
        let _ = tokio::time::timeout(std::time::Duration::from_millis(500), self.child.wait()).await;
        let _ = self.child.start_kill();
    }
}

/// Read stdout line by line, classify, and forward events in order.
async fn reader_loop(stdout: tokio::process::ChildStdout, tx: mpsc::Sender<AcpEvent>) {
    let mut lines = BufReader::new(stdout).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if line.trim().is_empty() {
                    continue;
                }
                if let Some(event) = classify(&line) {
                    if tx.send(event).await.is_err() {
                        break; // main loop is gone
                    }
                }
            }
            Ok(None) => break,  // EOF
            Err(_) => break,    // read error
        }
    }
    let _ = tx.send(AcpEvent::Closed).await;
}

/// Turn one JSON-RPC line into an [`AcpEvent`], or `None` to ignore it
/// (unknown notifications, including all `_kiro.dev/*` extension traffic).
fn classify(line: &str) -> Option<AcpEvent> {
    let value: Value = serde_json::from_str(line).ok()?;
    let obj = value.as_object()?;

    let has_id = obj.contains_key("id");
    let has_method = obj.contains_key("method");

    // Response: has an id and either a result or an error, no method.
    if has_id && !has_method && (obj.contains_key("result") || obj.contains_key("error")) {
        let id = obj.get("id").and_then(Value::as_u64)?;
        let result = if let Some(err) = obj.get("error") {
            RpcResult::Err(err.clone())
        } else {
            RpcResult::Ok(obj.get("result").cloned().unwrap_or(Value::Null))
        };
        return Some(AcpEvent::Response { id, result });
    }

    if has_method {
        let method = obj.get("method").and_then(Value::as_str)?.to_string();

        // Server -> client request: method + id.
        if has_id {
            let id = obj.get("id").cloned().unwrap_or(Value::Null);
            return Some(AcpEvent::ServerRequest { id, method });
        }

        // Notification. We only care about session/update; everything else
        // (notably `_kiro.dev/*`) is silently dropped.
        if method == "session/update" {
            let params = obj.get("params")?;
            match serde_json::from_value::<SessionNotification>(params.clone()) {
                Ok(note) => return Some(AcpEvent::Update(note.update)),
                Err(_) => return None, // malformed update: ignore rather than crash
            }
        }
        return None;
    }

    None
}

/// Drain the child's stderr so the pipe never blocks; optionally tee to a file.
async fn drain_stderr(stderr: tokio::process::ChildStderr, log_path: Option<String>) {
    use tokio::io::AsyncWriteExt as _;

    let mut lines = BufReader::new(stderr).lines();
    let mut file = match log_path {
        Some(path) => tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .ok(),
        None => None,
    };

    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(f) = file.as_mut() {
            let _ = f.write_all(line.as_bytes()).await;
            let _ = f.write_all(b"\n").await;
        }
    }
}
