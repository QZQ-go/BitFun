use crate::util::types::{Message, ToolDefinition};
use log::{error, warn};
use serde_json::{json, Value};

pub struct OpenAIResponsesMessageConverter;

impl OpenAIResponsesMessageConverter {
    pub fn convert_input(messages: Vec<Message>) -> Vec<Value> {
        messages
            .into_iter()
            .flat_map(Self::convert_single_message)
            .collect()
    }

    fn convert_single_message(msg: Message) -> Vec<Value> {
        match msg.role.as_str() {
            "system" | "user" => vec![Self::build_role_message_item(msg.role.as_str(), msg.content)],
            "assistant" => Self::build_assistant_items(msg),
            "tool" => vec![Self::build_tool_output_item(msg)],
            _ => {
                warn!("[OpenAI Responses] Unknown message role: {}", msg.role);
                vec![Self::build_role_message_item(msg.role.as_str(), msg.content)]
            }
        }
    }

    fn build_role_message_item(role: &str, content: Option<String>) -> Value {
        let text = content.unwrap_or_default();

        if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
            if parsed.is_array() {
                return json!({
                    "role": role,
                    "content": parsed,
                });
            }
        }

        json!({
            "role": role,
            "content": text,
        })
    }

    fn build_assistant_items(msg: Message) -> Vec<Value> {
        let mut items = Vec::new();

        if let Some(text) = msg.content {
            if !text.trim().is_empty() {
                items.push(json!({
                    "role": "assistant",
                    "content": text,
                }));
            }
        }

        if let Some(reasoning) = msg.reasoning_content {
            if !reasoning.trim().is_empty() {
                items.push(json!({
                    "role": "assistant",
                    "content": reasoning,
                }));
            }
        }

        if let Some(tool_calls) = msg.tool_calls {
            for tc in tool_calls {
                items.push(json!({
                    "type": "function_call",
                    "call_id": tc.id,
                    "name": tc.name,
                    "arguments": serde_json::to_string(&tc.arguments).unwrap_or_else(|e| {
                        error!(
                            "[OpenAI Responses] Failed to serialize tool arguments: {}",
                            e
                        );
                        "{}".to_string()
                    }),
                }));
            }
        }

        if items.is_empty() {
            items.push(json!({
                "role": "assistant",
                "content": " ",
            }));
        }

        items
    }

    fn build_tool_output_item(msg: Message) -> Value {
        let call_id = msg.tool_call_id.unwrap_or_default();
        let output = msg.content.unwrap_or_else(|| "Tool execution completed".to_string());

        json!({
            "type": "function_call_output",
            "call_id": call_id,
            "output": output,
        })
    }

    pub fn convert_tools(tools: Option<Vec<ToolDefinition>>) -> Option<Vec<Value>> {
        tools.map(|tool_defs| {
            tool_defs
                .into_iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    })
                })
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::OpenAIResponsesMessageConverter;
    use crate::util::types::{Message, ToolCall, ToolDefinition};
    use std::collections::HashMap;

    #[test]
    fn convert_input_maps_tool_messages_to_function_call_output() {
        let messages = vec![Message {
            role: "tool".to_string(),
            content: Some("{\"ok\":true}".to_string()),
            reasoning_content: None,
            thinking_signature: None,
            tool_calls: None,
            tool_call_id: Some("call_123".to_string()),
            name: Some("weather".to_string()),
        }];

        let input = OpenAIResponsesMessageConverter::convert_input(messages);
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["type"], "function_call_output");
        assert_eq!(input[0]["call_id"], "call_123");
    }

    #[test]
    fn convert_input_maps_assistant_tool_calls_to_function_call_items() {
        let mut args = HashMap::new();
        args.insert("city".to_string(), serde_json::json!("Beijing"));

        let messages = vec![Message {
            role: "assistant".to_string(),
            content: None,
            reasoning_content: None,
            thinking_signature: None,
            tool_calls: Some(vec![ToolCall {
                id: "call_1".to_string(),
                name: "get_weather".to_string(),
                arguments: args,
            }]),
            tool_call_id: None,
            name: None,
        }];

        let input = OpenAIResponsesMessageConverter::convert_input(messages);
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["type"], "function_call");
        assert_eq!(input[0]["name"], "get_weather");
    }

    #[test]
    fn convert_tools_outputs_responses_function_schema() {
        let tools = Some(vec![ToolDefinition {
            name: "get_weather".to_string(),
            description: "Get weather info".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "city": { "type": "string" }
                },
                "required": ["city"]
            }),
        }]);

        let converted = OpenAIResponsesMessageConverter::convert_tools(tools).unwrap();
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0]["type"], "function");
        assert_eq!(converted[0]["name"], "get_weather");
    }
}
