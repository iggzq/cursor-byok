//! Resolves Cursor model selections to configured provider models.
use crate::{
    cursor::protocol::proto::agent::v1 as pb,
    model::{
        parse_token_count, ModelLatency, ModelSpec, ReasoningSpec, SubagentKind,
        SubagentModelOverride,
    },
    store::Store,
    Error, Result,
};

pub fn requested_model(request: &pb::AgentRunRequest) -> Result<ModelSpec> {
    let details = request.model_details.as_ref();
    let model = if let Some(requested) = request.requested_model.as_ref() {
        from_requested(requested, details)?
    } else if let Some(model_id) = details
        .map(|model| model.model_id.as_str())
        .filter(|model| !model.is_empty())
    {
        ModelSpec {
            model_id: model_id.into(),
            display_name: details
                .map(|model| model.display_name.clone())
                .filter(|name| !name.is_empty()),
            reasoning: ReasoningSpec {
                enabled: details.is_some_and(|model| model.thinking_details.is_some()),
                effort: None,
            },
            latency: ModelLatency::Standard,
            max_output_tokens: None,
            context_window_tokens: None,
            supports_image_generation: false,
            extra_params: serde_json::json!({}),
        }
    } else {
        return Err(Error::Protocol("Cursor Run does not select a model".into()));
    };
    Ok(model)
}

/// Resolves Cursor's model selection to the configured model hash before the
/// model is used by the provider-independent run pipeline.
pub async fn resolve_requested_model(
    request: &pb::AgentRunRequest,
    store: &Store,
) -> Result<ModelSpec> {
    let mut model = requested_model(request)?;
    if let Some(configured) = store
        .resolve_model(&model.model_id, request.conversation_id.as_deref())
        .await?
    {
        model.model_id = configured.model_hash.clone();
        configured.configure(&mut model);
    }
    Ok(model)
}

pub fn overrides(
    request: &pb::AgentRunRequest,
) -> Result<Vec<(SubagentKind, SubagentModelOverride)>> {
    request
        .subagent_model_overrides
        .iter()
        .map(|value| {
            use pb::subagent_model_override::Selection;
            let kind = subagent_kind(&value.subagent_type);
            let selection = match value.selection.as_ref() {
                Some(Selection::Model(model)) => {
                    if model.model_id == "default" {
                        SubagentModelOverride::Inherit
                    } else {
                        SubagentModelOverride::Explicit(from_requested(model, None)?)
                    }
                }
                Some(Selection::Inherit(true)) => SubagentModelOverride::Inherit,
                Some(Selection::Disabled(true)) => SubagentModelOverride::Disabled,
                None | Some(Selection::Inherit(false) | Selection::Disabled(false)) => {
                    return Err(Error::Protocol(format!(
                        "Cursor subagent model override {} has no active selection",
                        value.subagent_type
                    )))
                }
            };
            Ok((kind, selection))
        })
        .collect()
}

pub fn subagent_kind(value: &str) -> SubagentKind {
    if value == "generalPurpose" {
        SubagentKind::GeneralPurpose
    } else {
        SubagentKind::Named(value.into())
    }
}

fn from_requested(
    model: &pb::RequestedModel,
    details: Option<&pb::ModelDetails>,
) -> Result<ModelSpec> {
    let mut spec = ModelSpec {
        model_id: model.model_id.clone(),
        display_name: details
            .map(|model| model.display_name.clone())
            .filter(|name| !name.is_empty()),
        reasoning: ReasoningSpec {
            enabled: model.max_mode
                || details.is_some_and(|model| model.thinking_details.is_some()),
            effort: None,
        },
        latency: ModelLatency::Standard,
        max_output_tokens: None,
        context_window_tokens: None,
        supports_image_generation: false,
        extra_params: serde_json::json!({}),
    };
    for parameter in &model.parameters {
        match parameter.id.as_str() {
            "effort" | "reasoning" => {
                let effort = parameter.value.trim();
                spec.reasoning.effort =
                    (effort != "none" && !effort.is_empty()).then(|| effort.to_string());
                spec.reasoning.enabled |= spec.reasoning.effort.is_some();
            }
            "thinking" => spec.reasoning.enabled |= parse_bool(parameter)?,
            "fast" => {
                if parse_bool(parameter)? {
                    spec.latency = ModelLatency::Fast;
                }
            }
            "context" => {
                spec.context_window_tokens =
                    Some(parse_token_count(&parameter.value).ok_or_else(|| {
                        Error::Protocol(format!(
                            "invalid Cursor context token count: {}",
                            parameter.value
                        ))
                    })?);
            }
            _ => {}
        }
    }
    Ok(spec)
}

fn parse_bool(parameter: &pb::requested_model::ModelParameterValue) -> Result<bool> {
    match parameter.value.as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(Error::Protocol(format!(
            "invalid Cursor boolean model parameter {}={}",
            parameter.id, parameter.value
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ModelConfigInput, ModelType, OPENAI_CHAT_ENDPOINT};

    fn model_input() -> ModelConfigInput {
        ModelConfigInput {
            sort_order: 0,
            display_name: "Test Model".into(),
            group_name: None,
            model_type: ModelType::OpenAi,
            base_url: "https://example.com/v1/chat/completions".into(),
            use_full_url: true,
            api_key: "test-key".into(),
            tooltip_data: "Test Model".into(),
            model_id: "test-model".into(),
            reasoning_effort: None,
            openai_endpoint: OPENAI_CHAT_ENDPOINT.into(),
            openai_extra_params_enabled: false,
            openai_extra_params: serde_json::json!({}),
            custom_headers_enabled: false,
            custom_headers: serde_json::json!({}),
            anthropic_extra_params_enabled: false,
            anthropic_extra_params: serde_json::json!({}),
            context_window_tokens: None,
            max_completion_tokens: None,
            anthropic_max_tokens: None,
            anthropic_thinking_effort: None,
            thinking_budget_tokens: None,
        }
    }

    #[test]
    fn ignores_unknown_cursor_model_parameters() {
        let requested = pb::RequestedModel {
            model_id: "test-model".into(),
            parameters: vec![pb::requested_model::ModelParameterValue {
                id: "optimize_for".into(),
                value: "quality".into(),
            }],
            ..Default::default()
        };

        let model = from_requested(&requested, None).expect("unknown parameter should be ignored");

        assert_eq!(model.model_id, "test-model");
        assert_eq!(model.latency, ModelLatency::Standard);
        assert!(!model.reasoning.enabled);
    }

    #[tokio::test]
    async fn resolves_cursor_model_alias_to_the_configured_hash() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::connect(&format!(
            "sqlite://{}",
            directory.path().join("test.db").display()
        ))
        .await
        .unwrap();
        let configured = store.create_model(&model_input()).await.unwrap();
        let request = pb::AgentRunRequest {
            conversation_id: Some("conversation".into()),
            requested_model: Some(pb::RequestedModel {
                model_id: "test-model".into(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let model = resolve_requested_model(&request, &store).await.unwrap();

        assert_eq!(model.model_id, configured.model_hash);
        assert_eq!(model.display_name.as_deref(), Some("Test Model"));
    }

    #[tokio::test]
    async fn resolves_cursor_default_to_the_configured_hash() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::connect(&format!(
            "sqlite://{}",
            directory.path().join("test.db").display()
        ))
        .await
        .unwrap();
        let configured = store.create_model(&model_input()).await.unwrap();
        let request = pb::AgentRunRequest {
            conversation_id: Some("conversation".into()),
            requested_model: Some(pb::RequestedModel {
                model_id: "default".into(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let model = resolve_requested_model(&request, &store).await.unwrap();

        assert_eq!(model.model_id, configured.model_hash);
    }
}
