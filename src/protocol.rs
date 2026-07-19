//! ACP / JSON-RPC message types.
//!
//! We only model the handful of messages kiro-tty actually uses. Everything
//! else on the wire (notably Kiro's `_kiro.dev/*` extension notifications) is
//! deliberately left untyped and ignored by the transport layer.
//!
//! Design note: outbound *params* are strongly typed for clarity and
//! correctness, while inbound payloads are read via `serde_json::Value` and
//! narrow structs. ACP responses carry extra fields we don't care about
//! (`modes`, `models`, capability maps), and typing them fully would be churn
//! for no benefit. Serde ignores unknown fields by default, so narrow structs
//! stay forward-compatible as Kiro evolves.
//!
//! Why not the official `agent-client-protocol-schema` crate? We evaluated it
//! (2026-07-19). It is well-made and its versioned `v1`/`v2` modules are a nice
//! future-proofing story, but adopting it added 25 transitive crates (+71% to
//! the dependency tree), including a *mandatory* `schemars` JSON-Schema
//! generator we would never call. The future-proofing that actually matters to
//! us — surviving *additive* protocol growth — is already handled here by
//! ignoring unknown fields plus the `SessionUpdate::Other` fallback, at zero
//! dependency cost. The message set is also effectively frozen: kiro-tty runs
//! on dumb terminals, so it will never grow image prompts, permission-request
//! schemas, or plan panels. Maintaining ~150 stable lines beats carrying that
//! tree. Revisit only if the modelled surface ever genuinely grows.

use serde::{Deserialize, Serialize};

/// The ACP protocol version kiro-tty speaks. Verified against Kiro CLI 2.13.0,
/// which advertised `protocolVersion: 1`.
pub const PROTOCOL_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// initialize
// ---------------------------------------------------------------------------

/// Params for the `initialize` request.
///
/// We advertise *no* client-provided filesystem capabilities: Kiro should use
/// its own host-local read/write/search/shell tools rather than delegating file
/// access back to us. This matches the project's "the Linux box does the real
/// work" principle.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub protocol_version: u32,
    pub client_capabilities: ClientCapabilities,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    pub fs: FsCapabilities,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsCapabilities {
    pub read_text_file: bool,
    pub write_text_file: bool,
}

impl Default for InitializeParams {
    fn default() -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            client_capabilities: ClientCapabilities {
                fs: FsCapabilities {
                    read_text_file: false,
                    write_text_file: false,
                },
            },
        }
    }
}

/// The subset of the `initialize` response we surface at startup.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    #[serde(default)]
    pub protocol_version: u32,
    #[serde(default)]
    pub agent_info: Option<AgentInfo>,
}

#[derive(Debug, Deserialize)]
pub struct AgentInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
}

// ---------------------------------------------------------------------------
// session/new
// ---------------------------------------------------------------------------

/// Params for the `session/new` request.
///
/// `cwd` must be an absolute path; it establishes the working context for the
/// agent's relative-path tool calls.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionParams {
    pub cwd: String,
    pub mcp_servers: Vec<serde_json::Value>,
}

/// We only need the session id back; `modes`/`models` are ignored.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionResult {
    pub session_id: String,
}

// ---------------------------------------------------------------------------
// session/prompt
// ---------------------------------------------------------------------------

/// Params for the `session/prompt` request. The prompt is a list of content
/// blocks; for line-oriented input we send a single text block.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptParams {
    pub session_id: String,
    pub prompt: Vec<ContentBlock>,
}

/// A single content block. ACP text blocks are `{ "type": "text", "text": ... }`.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
}

impl ContentBlock {
    pub fn text(s: impl Into<String>) -> Self {
        ContentBlock::Text { text: s.into() }
    }
}

/// The `session/prompt` response carries the reason the turn ended.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptResult {
    #[serde(default)]
    pub stop_reason: String,
}

// ---------------------------------------------------------------------------
// session/cancel  (notification, not a request)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelParams {
    pub session_id: String,
}

// ---------------------------------------------------------------------------
// session/update  (server -> client notification)
// ---------------------------------------------------------------------------

/// The `params` of a `session/update` notification.
///
/// The interesting payload is the inner `update` object, which is a tagged
/// union keyed by `sessionUpdate`. We parse it into [`SessionUpdate`].
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNotification {
    #[allow(dead_code)]
    #[serde(default)]
    pub session_id: String,
    pub update: SessionUpdate,
}

/// The kinds of `session/update` we care about. Unknown variants deserialize to
/// [`SessionUpdate::Other`] via the `#[serde(other)]`-style fallback so new Kiro
/// update types never crash the client.
#[derive(Debug, Deserialize)]
#[serde(tag = "sessionUpdate", rename_all = "snake_case")]
pub enum SessionUpdate {
    /// Streamed assistant text.
    AgentMessageChunk {
        content: ContentValue,
    },
    /// Assistant "thinking" text. Hidden by default in the renderer.
    AgentThoughtChunk {
        #[allow(dead_code)]
        content: ContentValue,
    },
    /// A tool invocation has started.
    ToolCall(ToolCall),
    /// A tool invocation changed status. Parsed but not yet rendered.
    #[allow(dead_code)]
    ToolCallUpdate(ToolCall),
    /// Any other update type (plan, user_message_chunk, available_commands, ...).
    /// Captured but not rendered in the MVP.
    #[serde(other)]
    Other,
}

/// A content value inside a chunk. ACP uses `{ "type": "text", "text": ... }`;
/// we tolerate a bare string too, just in case.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ContentValue {
    Structured {
        #[serde(default)]
        text: String,
    },
    Plain(String),
}

impl ContentValue {
    pub fn as_text(&self) -> &str {
        match self {
            ContentValue::Structured { text } => text,
            ContentValue::Plain(s) => s,
        }
    }
}

/// A tool call / tool call update. Fields are optional because `tool_call` and
/// `tool_call_update` populate different subsets. `tool_call_id` and `status`
/// are reserved for future tool-status and permission rendering.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    #[allow(dead_code)]
    #[serde(default)]
    pub tool_call_id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub status: Option<String>,
}
