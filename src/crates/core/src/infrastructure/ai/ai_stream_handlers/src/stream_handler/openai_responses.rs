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

pub async fn handle_openai_responses_stream(
    response: Response,
    tx_event: mpsc::UnboundedSender<Result<UnifiedResponse>>,
    tx_raw_sse: Option<mpsc::UnboundedSender<String>>,
) {
    let mut stream = response.bytes_stream().eventsource();
    let idle_timeout = Duration::from_secs(600);
    let mut call_name_by_id: HashMap<String, String> = HashMap::new();

    loop {
        let sse_event = timeout(idle_timeout, stream.next()).await;
        let sse = match sse_event {
            Ok(Some(Ok(sse))) => sse,
            Ok(None) => {
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

                if let (Some(call_id), Some(name)) = (
                    event.item.call_id.as_ref().filter(|id| !id.is_empty()),
                    event.item.name.as_ref().filter(|name| !name.is_empty()),
                ) {
                    call_name_by_id.insert(call_id.to_string(), name.to_string());
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

                let name = call_name_by_id.get(&event.call_id).cloned();
                let unified = event.into_unified_response_with_name(name);
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

                if let Some(name) = event.name.as_ref().filter(|name| !name.is_empty()) {
                    call_name_by_id.insert(event.call_id.clone(), name.to_string());
                }

                let _ = tx_event.send(Ok(event.into_unified_response()));
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
