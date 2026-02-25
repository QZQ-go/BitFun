use super::unified::{UnifiedResponse, UnifiedTokenUsage, UnifiedToolCall};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct OpenAIResponsesOutputTextDeltaEvent {
    pub delta: String,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIResponsesFunctionCallArgumentsDeltaEvent {
    pub call_id: String,
    pub delta: String,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIResponsesFunctionCallArgumentsDoneEvent {
    pub call_id: String,
    pub name: Option<String>,
    pub arguments: String,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIResponsesOutputItemEvent {
    pub item: OpenAIResponsesOutputItem,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIResponsesOutputItem {
    #[serde(rename = "type")]
    pub item_type: String,
    pub call_id: Option<String>,
    pub name: Option<String>,
    pub arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIResponsesCompletedEvent {
    pub response: OpenAIResponsesResponse,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIResponsesResponse {
    pub status: Option<String>,
    pub usage: Option<OpenAIResponsesUsage>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIResponsesUsage {
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub total_tokens: Option<u32>,
    pub input_token_details: Option<OpenAIResponsesInputTokenDetails>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIResponsesInputTokenDetails {
    pub cached_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIResponsesFailedEvent {
    pub response: OpenAIResponsesFailureResponse,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIResponsesFailureResponse {
    pub status: Option<String>,
    pub error: Option<OpenAIResponsesErrorPayload>,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIResponsesErrorEvent {
    pub error: OpenAIResponsesErrorPayload,
}

#[derive(Debug, Deserialize)]
pub struct OpenAIResponsesErrorPayload {
    pub message: Option<String>,
    #[serde(rename = "type")]
    pub error_type: Option<String>,
}

impl OpenAIResponsesUsage {
    pub fn into_unified_usage(self) -> Option<UnifiedTokenUsage> {
        let prompt_tokens = self.input_tokens.unwrap_or(0);
        let completion_tokens = self.output_tokens.unwrap_or(0);
        let total_tokens = self
            .total_tokens
            .unwrap_or(prompt_tokens.saturating_add(completion_tokens));

        if prompt_tokens == 0 && completion_tokens == 0 && total_tokens == 0 {
            return None;
        }

        Some(UnifiedTokenUsage {
            prompt_token_count: prompt_tokens,
            candidates_token_count: completion_tokens,
            total_token_count: total_tokens,
            cached_content_token_count: self
                .input_token_details
                .and_then(|details| details.cached_tokens),
        })
    }
}

impl OpenAIResponsesCompletedEvent {
    pub fn into_unified_response(self) -> UnifiedResponse {
        UnifiedResponse {
            text: None,
            reasoning_content: None,
            thinking_signature: None,
            tool_call: None,
            usage: self
                .response
                .usage
                .and_then(|usage| usage.into_unified_usage()),
            finish_reason: self
                .response
                .status
                .map(map_response_status_to_finish_reason),
        }
    }
}

impl OpenAIResponsesOutputItemEvent {
    pub fn into_tool_call_unified_response(self) -> Option<UnifiedResponse> {
        if self.item.item_type != "function_call" {
            return None;
        }

        let tool_call = UnifiedToolCall {
            id: self.item.call_id,
            name: self.item.name,
            arguments: self.item.arguments,
        };

        Some(UnifiedResponse {
            tool_call: Some(tool_call),
            ..Default::default()
        })
    }
}

impl OpenAIResponsesFunctionCallArgumentsDeltaEvent {
    pub fn into_unified_response_with_name(self, name: Option<String>) -> UnifiedResponse {
        UnifiedResponse {
            text: None,
            reasoning_content: None,
            thinking_signature: None,
            tool_call: Some(UnifiedToolCall {
                id: Some(self.call_id),
                name,
                arguments: Some(self.delta),
            }),
            usage: None,
            finish_reason: None,
        }
    }
}

impl OpenAIResponsesFunctionCallArgumentsDoneEvent {
    pub fn into_unified_response(self) -> UnifiedResponse {
        UnifiedResponse {
            text: None,
            reasoning_content: None,
            thinking_signature: None,
            tool_call: Some(UnifiedToolCall {
                id: Some(self.call_id),
                name: self.name,
                arguments: Some(self.arguments),
            }),
            usage: None,
            finish_reason: None,
        }
    }
}

pub fn extract_responses_event_type(event_json: &Value) -> Option<&str> {
    event_json.get("type").and_then(|value| value.as_str())
}

pub fn extract_responses_error_message(event_json: &Value) -> Option<String> {
    let error = event_json.get("error")?;

    if let Some(message) = error.get("message").and_then(|value| value.as_str()) {
        return Some(message.to_string());
    }

    if let Some(message) = error.as_str() {
        return Some(message.to_string());
    }

    Some("An error occurred during responses streaming".to_string())
}

fn map_response_status_to_finish_reason(status: String) -> String {
    match status.as_str() {
        "completed" => "stop".to_string(),
        "cancelled" => "cancelled".to_string(),
        "failed" => "error".to_string(),
        "incomplete" => "incomplete".to_string(),
        _ => status,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        extract_responses_error_message, extract_responses_event_type,
        OpenAIResponsesCompletedEvent,
    };

    #[test]
    fn maps_completed_event_usage_and_finish_reason() {
        let raw = r#"{
            "response": {
                "status": "completed",
                "usage": {
                    "input_tokens": 13,
                    "output_tokens": 8,
                    "total_tokens": 21,
                    "input_token_details": {
                        "cached_tokens": 5
                    }
                }
            }
        }"#;

        let event: OpenAIResponsesCompletedEvent =
            serde_json::from_str(raw).expect("valid completed event");
        let unified = event.into_unified_response();

        assert_eq!(unified.finish_reason.as_deref(), Some("stop"));
        let usage = unified.usage.expect("usage should exist");
        assert_eq!(usage.prompt_token_count, 13);
        assert_eq!(usage.candidates_token_count, 8);
        assert_eq!(usage.total_token_count, 21);
        assert_eq!(usage.cached_content_token_count, Some(5));
    }

    #[test]
    fn extracts_event_type_and_error_message() {
        let event = serde_json::json!({
            "type": "response.output_text.delta",
            "error": {
                "message": "provider error"
            }
        });

        assert_eq!(
            extract_responses_event_type(&event),
            Some("response.output_text.delta")
        );
        assert_eq!(
            extract_responses_error_message(&event).as_deref(),
            Some("provider error")
        );
    }
}
