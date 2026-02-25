use crate::service::config::types::AIModelConfig;
use log::warn;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// AI client configuration (for AI requests)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AIConfig {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub format: String,
    pub context_window: u32,
    pub max_tokens: Option<u32>,
    pub enable_thinking_process: bool,
    pub support_preserved_thinking: bool,
    pub custom_headers: Option<std::collections::HashMap<String, String>>,
    /// "replace" (default) or "merge" (defaults first, then custom)
    pub custom_headers_mode: Option<String>,
    pub skip_ssl_verify: bool,
    /// Custom JSON overriding default request body fields
    pub custom_request_body: Option<serde_json::Value>,
}

impl TryFrom<AIModelConfig> for AIConfig {
    type Error = String;
    fn try_from(other: AIModelConfig) -> Result<Self, <Self as TryFrom<AIModelConfig>>::Error> {
        // Parse custom request body (convert JSON string to serde_json::Value)
        let custom_request_body = if let Some(body_str) = &other.custom_request_body {
            let body_str = body_str.trim();
            if body_str.is_empty() {
                None
            } else {
                match serde_json::from_str::<serde_json::Value>(body_str) {
                    Ok(value) => Some(value),
                    Err(e) => {
                        warn!(
                            "Failed to parse custom_request_body: {}, config: {}",
                            e, other.name
                        );
                        None
                    }
                }
            }
        } else {
            None
        };

        let custom_headers = other.custom_headers.and_then(|headers| {
            let mut sanitized = HashMap::new();
            for (key, value) in headers {
                let key = key.trim();
                if key.is_empty() {
                    warn!(
                        "Ignoring custom header with empty name: config={} value={}",
                        other.name, value
                    );
                    continue;
                }

                sanitized.insert(key.to_string(), value.trim().to_string());
            }

            if sanitized.is_empty() {
                None
            } else {
                Some(sanitized)
            }
        });

        let custom_headers_mode = other.custom_headers_mode.and_then(|mode| {
            let mode = mode.trim().to_string();
            if mode.is_empty() {
                None
            } else {
                Some(mode)
            }
        });

        Ok(AIConfig {
            name: other.name.trim().to_string(),
            base_url: other.base_url.trim().to_string(),
            api_key: other.api_key.trim().to_string(),
            model: other.model_name.trim().to_string(),
            format: other.provider.trim().to_string(),
            context_window: other.context_window.unwrap_or(128128),
            max_tokens: other.max_tokens,
            enable_thinking_process: other.enable_thinking_process,
            support_preserved_thinking: other.support_preserved_thinking,
            custom_headers,
            custom_headers_mode,
            skip_ssl_verify: other.skip_ssl_verify,
            custom_request_body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::AIConfig;
    use crate::service::config::types::{AIModelConfig, ModelCapability, ModelCategory};
    use std::collections::HashMap;

    fn sample_model_config() -> AIModelConfig {
        AIModelConfig {
            id: "id_1".to_string(),
            name: "  Test Model  ".to_string(),
            provider: "  openai_responses  ".to_string(),
            model_name: "  gpt-5.2  ".to_string(),
            base_url: "  https://api.wecodemaster.com/v1  ".to_string(),
            api_key: "  sk-test-key\n  ".to_string(),
            context_window: Some(128000),
            max_tokens: Some(2048),
            temperature: None,
            top_p: None,
            frequency_penalty: None,
            presence_penalty: None,
            enabled: true,
            category: ModelCategory::GeneralChat,
            capabilities: vec![ModelCapability::TextChat],
            recommended_for: vec![],
            metadata: None,
            enable_thinking_process: false,
            support_preserved_thinking: false,
            custom_headers: None,
            custom_headers_mode: None,
            skip_ssl_verify: false,
            custom_request_body: None,
        }
    }

    #[test]
    fn try_from_model_config_trims_critical_string_fields() {
        let config = sample_model_config();
        let ai_config = AIConfig::try_from(config).expect("conversion should succeed");

        assert_eq!(ai_config.name, "Test Model");
        assert_eq!(ai_config.format, "openai_responses");
        assert_eq!(ai_config.model, "gpt-5.2");
        assert_eq!(ai_config.base_url, "https://api.wecodemaster.com/v1");
        assert_eq!(ai_config.api_key, "sk-test-key");
    }

    #[test]
    fn try_from_model_config_sanitizes_custom_headers_and_mode() {
        let mut config = sample_model_config();
        let mut headers = HashMap::new();
        headers.insert(
            "  X-Test-Header  ".to_string(),
            "  some-value  ".to_string(),
        );
        headers.insert("   ".to_string(), "ignored".to_string());
        config.custom_headers = Some(headers);
        config.custom_headers_mode = Some("  merge  ".to_string());

        let ai_config = AIConfig::try_from(config).expect("conversion should succeed");

        let headers = ai_config.custom_headers.expect("headers should exist");
        assert_eq!(headers.len(), 1);
        assert_eq!(
            headers.get("X-Test-Header").map(String::as_str),
            Some("some-value")
        );
        assert_eq!(ai_config.custom_headers_mode.as_deref(), Some("merge"));
    }

    #[test]
    fn try_from_model_config_ignores_blank_custom_request_body() {
        let mut config = sample_model_config();
        config.custom_request_body = Some("   ".to_string());

        let ai_config = AIConfig::try_from(config).expect("conversion should succeed");
        assert!(ai_config.custom_request_body.is_none());
    }
}
