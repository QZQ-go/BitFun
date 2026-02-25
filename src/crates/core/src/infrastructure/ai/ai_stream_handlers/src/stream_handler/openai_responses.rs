use crate::types::openai_responses::{
    extract_responses_error_message, extract_responses_event_type, OpenAIResponsesCompletedEvent,
    OpenAIResponsesErrorEvent, OpenAIResponsesFailedEvent,
    OpenAIResponsesFunctionCallArgumentsDeltaEvent, OpenAIResponsesFunctionCallArgumentsDoneEvent,
    OpenAIResponsesOutputItemEvent, OpenAIResponsesOutputTextDeltaEvent,
};
use crate::types::unified::UnifiedResponse;
use anyhow::{anyhow, Result};
use eventsource_stream::Eventsource;
use futures::StreamExt;
use log::{error, trace, warn};
use reqwest::Response;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::timeout;

fn resolve_call_id(
    call_id: Option<&str>,
    item_id: Option<&str>,
    call_id_by_item_id: &HashMap<String, String>,
) -> Option<String> {
    if let Some(call_id) = call_id.filter(|id| !id.is_empty()) {
        return Some(call_id.to_string());
    }

    if let Some(item_id) = item_id.filter(|id| !id.is_empty()) {
        return call_id_by_item_id
            .get(item_id)
            .cloned()
            .or_else(|| Some(item_id.to_string()));
    }

    None
}

fn should_treat_stream_close_as_success(
    saw_response_completed: bool,
    saw_meaningful_output: bool,
) -> bool {
    saw_response_completed || saw_meaningful_output
}

pub async fn handle_openai_responses_stream(
    response: Response,
    tx_event: mpsc::UnboundedSender<Result<UnifiedResponse>>,
    tx_raw_sse: Option<mpsc::UnboundedSender<String>>,
) {
    let mut stream = response.bytes_stream().eventsource();
    let idle_timeout = Duration::from_secs(600);
    let mut call_name_by_id: HashMap<String, String> = HashMap::new();
    let mut call_id_by_item_id: HashMap<String, String> = HashMap::new();
    let mut saw_response_completed = false;
    let mut saw_meaningful_output = false;

    loop {
        let sse_event = timeout(idle_timeout, stream.next()).await;
        let sse = match sse_event {
            Ok(Some(Ok(sse))) => sse,
            Ok(None) => {
                if should_treat_stream_close_as_success(
                    saw_response_completed,
                    saw_meaningful_output,
                ) {
                    if !saw_response_completed {
                        warn!(
                            "Responses SSE closed without response.completed; treating as success due to prior output"
                        );
                    }
                    return;
                }

                let error_msg = "Responses SSE stream closed before response completed";
                error!("{}", error_msg);
                let _ = tx_event.send(Err(anyhow!(error_msg)));
                return;
            }
            Ok(Some(Err(e))) => {
                let error_msg = format!("Responses SSE stream error: {}", e);
                error!("{}", error_msg);
                let _ = tx_event.send(Err(anyhow!(error_msg)));
                return;
            }
            Err(_) => {
                let error_msg = format!(
                    "Responses SSE stream timeout after {}s",
                    idle_timeout.as_secs()
                );
                error!("{}", error_msg);
                let _ = tx_event.send(Err(anyhow!(error_msg)));
                return;
            }
        };

        let raw = sse.data;
        trace!("OpenAI Responses SSE: {:?}", raw);
        if let Some(ref tx) = tx_raw_sse {
            let _ = tx.send(raw.clone());
        }
        if raw == "[DONE]" {
            return;
        }

        let event_json: Value = match serde_json::from_str(&raw) {
            Ok(json) => json,
            Err(e) => {
                let error_msg = format!("Responses SSE parsing error: {}, data: {}", e, &raw);
                error!("{}", error_msg);
                let _ = tx_event.send(Err(anyhow!(error_msg)));
                return;
            }
        };

        if let Some(api_error_message) = extract_responses_error_message(&event_json) {
            let error_msg = format!(
                "Responses SSE API error: {}, data: {}",
                api_error_message, raw
            );
            error!("{}", error_msg);
            let _ = tx_event.send(Err(anyhow!(error_msg)));
            return;
        }

        let event_type = extract_responses_event_type(&event_json)
            .map(ToOwned::to_owned)
            .or_else(|| {
                if sse.event.is_empty() {
                    None
                } else {
                    Some(sse.event)
                }
            });

        let Some(event_type) = event_type else {
            warn!("Skipping responses SSE event without type field: {}", raw);
            continue;
        };

        match event_type.as_str() {
            "response.output_text.delta" => {
                let event: OpenAIResponsesOutputTextDeltaEvent =
                    match serde_json::from_value(event_json) {
                        Ok(event) => event,
                        Err(e) => {
                            let error_msg =
                                format!("Responses text-delta schema error: {}, data: {}", e, &raw);
                            error!("{}", error_msg);
                            let _ = tx_event.send(Err(anyhow!(error_msg)));
                            return;
                        }
                    };

                if !event.delta.is_empty() {
                    saw_meaningful_output = true;
                }

                let _ = tx_event.send(Ok(UnifiedResponse {
                    text: Some(event.delta),
                    ..Default::default()
                }));
            }
            "response.output_item.added" | "response.output_item.done" => {
                let event: OpenAIResponsesOutputItemEvent = match serde_json::from_value(event_json)
                {
                    Ok(event) => event,
                    Err(e) => {
                        let error_msg =
                            format!("Responses output-item schema error: {}, data: {}", e, &raw);
                        error!("{}", error_msg);
                        let _ = tx_event.send(Err(anyhow!(error_msg)));
                        return;
                    }
                };

                saw_meaningful_output = true;

                if let (Some(call_id), Some(name)) = (
                    event.item.call_id.as_ref().filter(|id| !id.is_empty()),
                    event.item.name.as_ref().filter(|name| !name.is_empty()),
                ) {
                    call_name_by_id.insert(call_id.to_string(), name.to_string());
                }

                if let (Some(item_id), Some(call_id)) = (
                    event.item.id.as_ref().filter(|id| !id.is_empty()),
                    event.item.call_id.as_ref().filter(|id| !id.is_empty()),
                ) {
                    call_id_by_item_id.insert(item_id.to_string(), call_id.to_string());
                }

                if let Some(unified) = event.into_tool_call_unified_response() {
                    let _ = tx_event.send(Ok(unified));
                }
            }
            "response.function_call_arguments.delta" => {
                let event: OpenAIResponsesFunctionCallArgumentsDeltaEvent =
                    match serde_json::from_value(event_json) {
                        Ok(event) => event,
                        Err(e) => {
                            let error_msg = format!(
                                "Responses function-call-arguments delta schema error: {}, data: {}",
                                e, &raw
                            );
                            error!("{}", error_msg);
                            let _ = tx_event.send(Err(anyhow!(error_msg)));
                            return;
                        }
                    };

                if !event.delta.is_empty() {
                    saw_meaningful_output = true;
                }

                let resolved_call_id =
                    resolve_call_id(event.call_id.as_deref(), event.item_id.as_deref(), &call_id_by_item_id);

                let Some(resolved_call_id) = resolved_call_id else {
                    warn!(
                        "Skipping function_call_arguments.delta without call_id/item_id: {}",
                        raw
                    );
                    continue;
                };

                let name = call_name_by_id.get(&resolved_call_id).cloned();
                let unified = event.into_unified_response_with_name(resolved_call_id, name);
                let _ = tx_event.send(Ok(unified));
            }
            "response.function_call_arguments.done" => {
                let event: OpenAIResponsesFunctionCallArgumentsDoneEvent =
                    match serde_json::from_value(event_json) {
                        Ok(event) => event,
                        Err(e) => {
                            let error_msg = format!(
                                "Responses function-call-arguments done schema error: {}, data: {}",
                                e, &raw
                            );
                            error!("{}", error_msg);
                            let _ = tx_event.send(Err(anyhow!(error_msg)));
                            return;
                        }
                    };

                if !event.arguments.is_empty() {
                    saw_meaningful_output = true;
                }

                let resolved_call_id =
                    resolve_call_id(event.call_id.as_deref(), event.item_id.as_deref(), &call_id_by_item_id);

                let Some(resolved_call_id) = resolved_call_id else {
                    warn!(
                        "Skipping function_call_arguments.done without call_id/item_id: {}",
                        raw
                    );
                    continue;
                };

                if let (Some(item_id), Some(call_id)) = (
                    event.item_id.as_ref().filter(|id| !id.is_empty()),
                    event.call_id.as_ref().filter(|id| !id.is_empty()),
                ) {
                    call_id_by_item_id.insert(item_id.to_string(), call_id.to_string());
                }

                if let Some(name) = event.name.as_ref().filter(|name| !name.is_empty()) {
                    call_name_by_id.insert(resolved_call_id.clone(), name.to_string());
                }

                let _ = tx_event.send(Ok(event.into_unified_response(resolved_call_id)));
            }
            "response.completed" => {
                let event: OpenAIResponsesCompletedEvent = match serde_json::from_value(event_json)
                {
                    Ok(event) => event,
                    Err(e) => {
                        let error_msg =
                            format!("Responses completed schema error: {}, data: {}", e, &raw);
                        error!("{}", error_msg);
                        let _ = tx_event.send(Err(anyhow!(error_msg)));
                        return;
                    }
                };

                saw_response_completed = true;

                let _ = tx_event.send(Ok(event.into_unified_response()));
            }
            "response.failed" => {
                let event: OpenAIResponsesFailedEvent = match serde_json::from_value(event_json) {
                    Ok(event) => event,
                    Err(e) => {
                        let error_msg =
                            format!("Responses failed schema error: {}, data: {}", e, &raw);
                        error!("{}", error_msg);
                        let _ = tx_event.send(Err(anyhow!(error_msg)));
                        return;
                    }
                };

                let status = event
                    .response
                    .status
                    .unwrap_or_else(|| "failed".to_string());
                let details = event
                    .response
                    .error
                    .and_then(|error| {
                        error
                            .message
                            .or(error.error_type)
                            .or(Some("Unknown responses failure".to_string()))
                    })
                    .unwrap_or_else(|| "Unknown responses failure".to_string());

                let _ = tx_event.send(Err(anyhow!(
                    "Responses SSE failed event: status={}, message={}",
                    status,
                    details
                )));
                return;
            }
            "error" => {
                let event: OpenAIResponsesErrorEvent = match serde_json::from_value(event_json) {
                    Ok(event) => event,
                    Err(e) => {
                        let error_msg =
                            format!("Responses error schema error: {}, data: {}", e, &raw);
                        error!("{}", error_msg);
                        let _ = tx_event.send(Err(anyhow!(error_msg)));
                        return;
                    }
                };

                let details = event
                    .error
                    .message
                    .or(event.error.error_type)
                    .unwrap_or_else(|| "Unknown responses error".to_string());

                let _ = tx_event.send(Err(anyhow!("Responses SSE error: {}", details)));
                return;
            }
            _ => {
                trace!(
                    "Ignoring unsupported responses SSE event type: {}",
                    event_type
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_call_id, should_treat_stream_close_as_success};
    use std::collections::HashMap;

    #[test]
    fn resolve_call_id_prefers_explicit_call_id() {
        let mut map = HashMap::new();
        map.insert("item_1".to_string(), "call_1".to_string());

        let resolved = resolve_call_id(Some("call_explicit"), Some("item_1"), &map);
        assert_eq!(resolved.as_deref(), Some("call_explicit"));
    }

    #[test]
    fn resolve_call_id_maps_item_id_to_call_id_when_available() {
        let mut map = HashMap::new();
        map.insert("item_1".to_string(), "call_1".to_string());

        let resolved = resolve_call_id(None, Some("item_1"), &map);
        assert_eq!(resolved.as_deref(), Some("call_1"));
    }

    #[test]
    fn resolve_call_id_falls_back_to_item_id_when_mapping_missing() {
        let map = HashMap::new();

        let resolved = resolve_call_id(None, Some("item_only"), &map);
        assert_eq!(resolved.as_deref(), Some("item_only"));
    }

    #[test]
    fn resolve_call_id_returns_none_when_no_identifier_present() {
        let map = HashMap::new();

        let resolved = resolve_call_id(None, None, &map);
        assert_eq!(resolved, None);
    }

    #[test]
    fn stream_close_with_completed_event_is_success() {
        assert!(should_treat_stream_close_as_success(true, false));
    }

    #[test]
    fn stream_close_with_meaningful_output_is_success() {
        assert!(should_treat_stream_close_as_success(false, true));
    }

    #[test]
    fn stream_close_without_terminal_or_output_is_error() {
        assert!(!should_treat_stream_close_as_success(false, false));
    }
}
