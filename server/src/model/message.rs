//! Defines canonical append-only Conversation Messages.
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ToolImageReference, ToolRoundId};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Prompt,
    User,
    Runtime,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolCallContent {
    pub index: usize,
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ToolResultContent {
    pub call_id: String,
    pub name: String,
    pub content: String,
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ToolImageReference>,
    #[serde(skip)]
    pub provider_parts: Vec<ContentPart>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ProviderReplayState {
    pub provider_kind: String,
    pub value: Value,
}

impl ProviderReplayState {
    const COMPOSITE_KIND: &'static str = "composite";

    pub fn combine(states: Vec<Self>) -> crate::Result<Option<Self>> {
        let mut parts = Vec::new();
        for state in states {
            if state.provider_kind == Self::COMPOSITE_KIND {
                for (provider_kind, value) in state.parts()? {
                    parts.push(Self {
                        provider_kind: provider_kind.into(),
                        value: value.clone(),
                    });
                }
            } else {
                parts.push(state);
            }
        }
        match parts.len() {
            0 => Ok(None),
            1 => Ok(parts.pop()),
            _ => Ok(Some(Self {
                provider_kind: Self::COMPOSITE_KIND.into(),
                value: serde_json::to_value(parts)?,
            })),
        }
    }

    pub fn parts(&self) -> crate::Result<Vec<(&str, &Value)>> {
        if self.provider_kind != Self::COMPOSITE_KIND {
            return Ok(vec![(self.provider_kind.as_str(), &self.value)]);
        }
        self.value
            .as_array()
            .ok_or_else(|| crate::Error::Protocol("composite replay state is not an array".into()))?
            .iter()
            .map(|part| {
                let provider_kind = part
                    .get("provider_kind")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        crate::Error::Protocol(
                            "composite replay state part is missing provider_kind".into(),
                        )
                    })?;
                let value = part.get("value").ok_or_else(|| {
                    crate::Error::Protocol("composite replay state part is missing value".into())
                })?;
                Ok((provider_kind, value))
            })
            .collect()
    }

    pub fn values_for(&self, provider_kind: &str) -> crate::Result<Vec<&Value>> {
        Ok(self
            .parts()?
            .into_iter()
            .filter_map(|(kind, value)| (kind == provider_kind).then_some(value))
            .collect())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text {
        text: String,
    },
    Image {
        mime_type: String,
        #[serde(with = "base64_bytes")]
        data: Vec<u8>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MessageContent {
    Parts {
        parts: Vec<ContentPart>,
    },
    Assistant {
        text: String,
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_round_id: Option<ToolRoundId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replay_state: Option<ProviderReplayState>,
        tool_calls: Vec<ToolCallContent>,
    },
    ToolResult(ToolResultContent),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CanonicalMessage {
    pub message_id: String,
    pub role: Role,
    pub origin: Origin,
    pub content: MessageContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_event_id: Option<String>,
}

impl CanonicalMessage {
    pub fn text(
        message_id: impl Into<String>,
        role: Role,
        origin: Origin,
        text: impl Into<String>,
    ) -> Self {
        Self {
            message_id: message_id.into(),
            role,
            origin,
            content: MessageContent::Parts {
                parts: vec![ContentPart::Text { text: text.into() }],
            },
            runtime_event_id: None,
        }
    }

    pub fn parts(
        message_id: impl Into<String>,
        role: Role,
        origin: Origin,
        parts: Vec<ContentPart>,
    ) -> Self {
        Self {
            message_id: message_id.into(),
            role,
            origin,
            content: MessageContent::Parts { parts },
            runtime_event_id: None,
        }
    }
}

mod base64_bytes {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(data: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&STANDARD.encode(data))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        STANDARD.decode(encoded).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeEvent {
    pub event_id: String,
    pub text: String,
}

impl RuntimeEvent {
    pub fn into_message(self) -> CanonicalMessage {
        CanonicalMessage {
            message_id: format!("runtime:{}", self.event_id),
            role: Role::User,
            origin: Origin::Runtime,
            content: MessageContent::Parts {
                parts: vec![ContentPart::Text { text: self.text }],
            },
            runtime_event_id: Some(self.event_id),
        }
    }
}
