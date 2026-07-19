//! kiro-tty: a dumb-terminal, line-oriented frontend for `kiro-cli acp`.
//!
//! Flow: spawn the agent -> `initialize` -> `session/new` -> a turn-based REPL
//! that streams sanitized, wrapped plain text back to the terminal. See the
//! module docs in `transport`, `render`, and `repl` for the moving parts.

mod protocol;
mod render;
mod repl;
mod transport;

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use tokio::io::{AsyncWriteExt, Stdout};

use protocol::{
    CancelParams, ContentBlock, InitializeParams, InitializeResult, NewSessionParams,
    NewSessionResult, PromptParams, PromptResult, SessionUpdate,
};
use render::{Newline, Renderer};
use repl::{parse, Command, InputLine, HELP_TEXT};
use transport::{AcpEvent, RpcResult, Transport};

/// Command-line configuration.
#[derive(Parser, Debug)]
#[command(
    name = "kiro-tty",
    about = "A dumb-terminal frontend for Kiro CLI over ACP",
    version
)]
struct Args {
    /// Working directory for the Kiro session (defaults to the current dir).
    #[arg(long)]
    cwd: Option<PathBuf>,

    /// Hard-wrap column width.
    #[arg(long, default_value_t = 80)]
    width: usize,

    /// Terminal newline convention: lf, crlf, or cr.
    #[arg(long, default_value_t = Newline::Lf)]
    newline: Newline,

    /// Force uppercase output (for hardware/character sets that need it).
    #[arg(long)]
    uppercase: bool,

    /// Kiro agent to use for the session.
    #[arg(long)]
    agent: Option<String>,

    /// Model id to use for the session.
    #[arg(long)]
    model: Option<String>,

    /// Disable trust-all demo mode (permission handling is not yet implemented,
    /// so disabling trust may cause turns to stall on approval requests).
    #[arg(long = "no-trust")]
    no_trust: bool,

    /// Write the agent's stderr to this file for debugging.
    #[arg(long)]
    log: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Resolve an absolute cwd; ACP requires it.
    let cwd = match args.cwd {
        Some(p) => p,
        None => std::env::current_dir().context("failed to read current directory")?,
    };
    let cwd = cwd
        .canonicalize()
        .with_context(|| format!("cwd does not exist: {}", cwd.display()))?;
    let cwd_str = cwd
        .to_str()
        .ok_or_else(|| anyhow!("cwd is not valid UTF-8"))?
        .to_string();

    let trust_all = !args.no_trust;

    let mut out = tokio::io::stdout();
    let mut renderer = Renderer::new(args.width, args.newline, args.uppercase);

    // Assemble `kiro-cli acp` arguments.
    let mut extra_args: Vec<String> = Vec::new();
    if trust_all {
        extra_args.push("--trust-all-tools".to_string());
    }
    if let Some(agent) = &args.agent {
        extra_args.push("--agent".to_string());
        extra_args.push(agent.clone());
    }
    if let Some(model) = &args.model {
        extra_args.push("--model".to_string());
        extra_args.push(model.clone());
    }

    let mut transport = Transport::spawn(&extra_args, args.log.as_deref())
        .context("could not start the Kiro agent")?;

    // ----- initialize -----
    let init_id = transport
        .request("initialize", InitializeParams::default())
        .await?;
    let init_result = wait_for_response(&mut transport, init_id)
        .await
        .context("initialize failed")?;
    let init: InitializeResult =
        serde_json::from_value(init_result).context("could not parse initialize result")?;

    // ----- session/new -----
    let session_id = new_session(&mut transport, &cwd_str).await?;

    // ----- startup banner -----
    let agent_name = init
        .agent_info
        .map(|a| format!("{} {}", a.name, a.version))
        .unwrap_or_else(|| "Kiro".to_string());
    write(&mut out, &renderer.ui_line("KIRO TTY")).await?;
    write(
        &mut out,
        &renderer.ui_line(&format!(
            "connected to {} (acp v{})",
            agent_name, init.protocol_version
        )),
    )
    .await?;
    write(&mut out, &renderer.ui_line(&format!("workspace: {cwd_str}"))).await?;
    if trust_all {
        write(
            &mut out,
            &renderer.ui_line("*** DANGER MODE: all tools auto-approved ***"),
        )
        .await?;
    }
    write(&mut out, &renderer.ui_line("type /help for commands")).await?;
    write(&mut out, &renderer.ui_line("READY.")).await?;

    // ----- REPL -----
    let mut stdin = repl::spawn_stdin_reader();
    let mut session_id = session_id;
    write_prompt(&mut out, &renderer).await?;

    loop {
        tokio::select! {
            maybe_line = stdin.recv() => {
                let Some(InputLine(line)) = maybe_line else {
                    // stdin closed (remote hangup / EOF). Exit cleanly.
                    write(&mut out, &renderer.ui_line("")).await?;
                    break;
                };
                match parse(&line) {
                    Command::Empty => {}
                    Command::Help => {
                        for l in HELP_TEXT.lines() {
                            write(&mut out, &renderer.ui_line(l)).await?;
                        }
                    }
                    Command::Quit => {
                        write(&mut out, &renderer.ui_line("BYE.")).await?;
                        break;
                    }
                    Command::New => {
                        session_id = new_session(&mut transport, &cwd_str).await?;
                        write(&mut out, &renderer.ui_line("(new conversation)")).await?;
                    }
                    Command::Cancel => {
                        write(&mut out, &renderer.ui_line("(nothing to cancel)")).await?;
                    }
                    Command::Unknown(cmd) => {
                        write(
                            &mut out,
                            &renderer.ui_line(&format!("unknown command: /{cmd} (try /help)")),
                        )
                        .await?;
                    }
                    Command::Prompt(text) => {
                        run_turn(&mut transport, &mut renderer, &mut out, &session_id, &text)
                            .await?;
                    }
                }
                write_prompt(&mut out, &renderer).await?;
            }
            _ = tokio::signal::ctrl_c() => {
                // Idle Ctrl-C: don't kill the shell by accident.
                write(&mut out, &renderer.ui_line("")).await?;
                write(&mut out, &renderer.ui_line("(use /quit to exit)")).await?;
                write_prompt(&mut out, &renderer).await?;
            }
        }
    }

    transport.shutdown().await;
    Ok(())
}

/// Send `session/new` and return the new session id.
async fn new_session(transport: &mut Transport, cwd: &str) -> Result<String> {
    let id = transport
        .request(
            "session/new",
            NewSessionParams {
                cwd: cwd.to_string(),
                mcp_servers: Vec::new(),
            },
        )
        .await?;
    let result = wait_for_response(transport, id)
        .await
        .context("session/new failed")?;
    let session: NewSessionResult =
        serde_json::from_value(result).context("could not parse session/new result")?;
    Ok(session.session_id)
}

/// Run one prompt turn: send the prompt, stream updates until the response, and
/// allow Ctrl-C to cancel mid-turn.
async fn run_turn(
    transport: &mut Transport,
    renderer: &mut Renderer,
    out: &mut Stdout,
    session_id: &str,
    text: &str,
) -> Result<()> {
    let prompt_id = transport
        .request(
            "session/prompt",
            PromptParams {
                session_id: session_id.to_string(),
                prompt: vec![ContentBlock::text(text)],
            },
        )
        .await?;

    let mut cancelled = false;

    loop {
        tokio::select! {
            event = transport.next_event() => {
                match event {
                    AcpEvent::Update(update) => {
                        render_update(renderer, out, &update).await?;
                    }
                    AcpEvent::Response { id, result } if id == prompt_id => {
                        // End of turn: flush any buffered partial word/line.
                        write(out, &renderer.flush()).await?;
                        match result {
                            RpcResult::Ok(value) => {
                                let pr: PromptResult =
                                    serde_json::from_value(value).unwrap_or(PromptResult {
                                        stop_reason: String::new(),
                                    });
                                if cancelled || pr.stop_reason == "cancelled" {
                                    write(out, &renderer.ui_line("(cancelled)")).await?;
                                } else if !pr.stop_reason.is_empty()
                                    && pr.stop_reason != "end_turn"
                                {
                                    write(
                                        out,
                                        &renderer
                                            .ui_line(&format!("(stopped: {})", pr.stop_reason)),
                                    )
                                    .await?;
                                }
                            }
                            RpcResult::Err(err) => {
                                let msg = err
                                    .get("message")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("unknown error");
                                write(out, &renderer.ui_line(&format!("ERROR: {msg}"))).await?;
                            }
                        }
                        break;
                    }
                    AcpEvent::Response { .. } => {
                        // A stale response to some earlier request; ignore.
                    }
                    AcpEvent::ServerRequest { id, method } => {
                        // We advertise no client capabilities and run trust-all,
                        // so we don't expect these. Decline politely so the agent
                        // doesn't wait forever.
                        let _ = transport
                            .respond_error(id, -32601, &format!("method not supported: {method}"))
                            .await;
                    }
                    AcpEvent::Closed => {
                        write(out, &renderer.flush()).await?;
                        write(out, &renderer.ui_line("(agent disconnected)")).await?;
                        return Err(anyhow!("Kiro agent closed the connection"));
                    }
                }
            }
            _ = tokio::signal::ctrl_c(), if !cancelled => {
                cancelled = true;
                let _ = transport
                    .notify(
                        "session/cancel",
                        CancelParams { session_id: session_id.to_string() },
                    )
                    .await;
                write(out, &renderer.ui_line("(cancelling...)")).await?;
            }
        }
    }

    Ok(())
}

/// Render a single `session/update` to the terminal.
async fn render_update(
    renderer: &mut Renderer,
    out: &mut Stdout,
    update: &SessionUpdate,
) -> Result<()> {
    match update {
        SessionUpdate::AgentMessageChunk { content } => {
            write(out, &renderer.feed(content.as_text())).await?;
        }
        SessionUpdate::ToolCall(tc) => {
            let title = tc.title.clone().unwrap_or_else(|| {
                tc.kind.clone().unwrap_or_else(|| "tool".to_string())
            });
            write(out, &renderer.ui_line(&format!("[TOOL] {title}"))).await?;
        }
        // Tool status changes, thinking, and everything else are intentionally
        // not rendered in the MVP to keep a slow terminal uncluttered.
        // This is the way it should stay anyway. 
        SessionUpdate::ToolCallUpdate(_)
        | SessionUpdate::AgentThoughtChunk { .. }
        | SessionUpdate::Other => {}
    }
    Ok(())
}

/// Wait for the response to a specific request id, ignoring notifications and
/// declining any interleaved server requests.
async fn wait_for_response(transport: &mut Transport, want_id: u64) -> Result<serde_json::Value> {
    loop {
        match transport.next_event().await {
            AcpEvent::Response { id, result } if id == want_id => match result {
                RpcResult::Ok(value) => return Ok(value),
                RpcResult::Err(err) => {
                    let msg = err
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("unknown error");
                    return Err(anyhow!("agent returned error: {msg}"));
                }
            },
            AcpEvent::ServerRequest { id, method } => {
                let _ = transport
                    .respond_error(id, -32601, &format!("method not supported: {method}"))
                    .await;
            }
            AcpEvent::Closed => return Err(anyhow!("agent closed the connection")),
            // Ignore updates and unrelated responses during setup.
            _ => {}
        }
    }
}

/// Write the input prompt (no trailing newline; the user types on this line).
async fn write_prompt(out: &mut Stdout, renderer: &Renderer) -> Result<()> {
    // A blank line before the prompt keeps turns visually separated.
    let s = format!("{nl}> ", nl = renderer.newline_str());
    out.write_all(s.as_bytes()).await?;
    out.flush().await?;
    Ok(())
}

/// Write already-rendered bytes to stdout and flush (streaming needs the flush).
async fn write(out: &mut Stdout, s: &str) -> Result<()> {
    if s.is_empty() {
        return Ok(());
    }
    out.write_all(s.as_bytes()).await?;
    out.flush().await?;
    Ok(())
}
