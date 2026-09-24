//! Conversation model: provider-neutral messages made of content blocks,
//! mirroring the Anthropic Messages shape (text / thinking / tool_use /
//! tool_result). Every provider adapter converts to and from this.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<Block>,
}

impl Message {
    pub fn user_text(t: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![Block::Text { text: t.into() }],
        }
    }

    pub fn assistant_text(t: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![Block::Text { text: t.into() }],
        }
    }

    /// All plain text in the message, joined.
    pub fn text(&self) -> String {
        let mut s = String::new();
        for b in &self.content {
            if let Block::Text { text } = b {
                if !s.is_empty() {
                    s.push('\n');
                }
                s.push_str(text);
            }
        }
        s
    }

    pub fn tool_uses(&self) -> Vec<(&str, &str, &Value)> {
        self.content
            .iter()
            .filter_map(|b| match b {
                Block::ToolUse { id, name, input } => Some((id.as_str(), name.as_str(), input)),
                _ => None,
            })
            .collect()
    }

    /// Rough size in tokens (≈4 chars per token) for budgeting before the
    /// provider reports real usage.
    pub fn approx_tokens(&self) -> u64 {
        let chars: usize = self
            .content
            .iter()
            .map(|b| match b {
                Block::Text { text } => text.len(),
                Block::Thinking { thinking, .. } => thinking.len(),
                Block::RedactedThinking { data } => data.len() / 4,
                Block::ToolUse { input, name, .. } => input.to_string().len() + name.len(),
                Block::ToolResult { content, .. } => content.len(),
            })
            .sum();
        (chars as u64 / 4).max(1)
    }
}

/// Token accounting reported by the provider.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
}

impl Usage {
    pub fn add(&mut self, o: &Usage) {
        self.input_tokens += o.input_tokens;
        self.output_tokens += o.output_tokens;
        self.cache_read_input_tokens += o.cache_read_input_tokens;
        self.cache_creation_input_tokens += o.cache_creation_input_tokens;
    }

    /// Everything that occupied the context window on this request.
    pub fn context_tokens(&self) -> u64 {
        self.input_tokens
            + self.cache_read_input_tokens
            + self.cache_creation_input_tokens
            + self.output_tokens
    }
}

/// A short, unique id for tool calls synthesised locally (text protocol).
pub fn new_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{:x}{:04x}", t & 0xffff_ffff_ffff, n & 0xffff)
}

/// RFC 4122 v4-shaped id (random enough for session files; no crypto use).
pub fn uuid() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CALLS: AtomicU64 = AtomicU64::new(0);
    let salt = CALLS
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut seed = salt
        ^ std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1)
        ^ (std::process::id() as u64).rotate_left(32);
    let mut next = || {
        // xorshift64*
        seed ^= seed >> 12;
        seed ^= seed << 25;
        seed ^= seed >> 27;
        seed.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let a = next();
    let b = next();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:x}{:03x}-{:012x}",
        (a >> 32) as u32,
        (a >> 16) as u16,
        (a & 0xfff) as u16,
        8 + (b >> 62) as u8,
        ((b >> 48) & 0xfff) as u16,
        b & 0xffff_ffff_ffff
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_roundtrip_as_anthropic_json() {
        let m = Message {
            role: Role::Assistant,
            content: vec![
                Block::Text { text: "hi".into() },
                Block::ToolUse {
                    id: "t1".into(),
                    name: "Read".into(),
                    input: serde_json::json!({"file_path":"a"}),
                },
            ],
        };
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(v["content"][1]["type"], "tool_use");
        assert_eq!(v["role"], "assistant");
        let back: Message = serde_json::from_value(v).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn uuids_look_like_v4_and_differ() {
        let a = uuid();
        let b = uuid();
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "4");
        assert_ne!(a, b);
    }
}
