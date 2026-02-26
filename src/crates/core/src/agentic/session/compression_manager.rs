//! Context Compression Manager
//!
//! Responsible for managing session context compression

use crate::agentic::core::{Message, MessageContent, MessageHelper, MessageRole};
use crate::agentic::persistence::PersistenceManager;
use crate::infrastructure::ai::{get_global_ai_client_factory, AIClient};
use crate::util::errors::{BitFunError, BitFunResult};
use crate::util::token_counter::TokenCounter;
use crate::util::types::Message as AIMessage;
use anyhow;
use dashmap::DashMap;
use log::{debug, trace, warn};
use std::sync::Arc;

/// Compression manager configuration
#[derive(Debug, Clone)]
pub struct CompressionConfig {
    pub enable_persistence: bool,
    pub keep_turns_ratio: f32,
    pub keep_last_turn_ratio: f32,
    pub single_request_max_tokens_ratio: f32,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            enable_persistence: true,
            keep_turns_ratio: 0.3,
            keep_last_turn_ratio: 0.4,
            single_request_max_tokens_ratio: 0.7,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TurnWithTokens {
    messages: Vec<Message>,
    tokens: usize,
}

impl TurnWithTokens {
    fn new(messages: Vec<Message>, tokens: usize) -> Self {
        Self { messages, tokens }
    }
}

/// Context compression manager
pub struct CompressionManager {
    /// Compressed message history (by session ID)
    compressed_histories: Arc<DashMap<String, Vec<Message>>>,
    /// Persistence manager
    persistence: Arc<PersistenceManager>,
    /// Configuration
    config: CompressionConfig,
}

impl CompressionManager {
    pub fn new(persistence: Arc<PersistenceManager>, config: CompressionConfig) -> Self {
        Self {
            compressed_histories: Arc::new(DashMap::new()),
            persistence,
            config,
        }
    }

    /// Create session compression history
    pub fn create_session(&self, session_id: &str) {
        self.compressed_histories
            .insert(session_id.to_string(), vec![]);
        debug!(
            "Created session compression history: session_id={}",
            session_id
        );
    }

    /// Add message (async, supports persistence)
    pub async fn add_message(&self, session_id: &str, message: Message) -> BitFunResult<()> {
        // 1. Add to memory
        if let Some(mut compressed) = self.compressed_histories.get_mut(session_id) {
            compressed.push(message.clone());
        } else {
            self.compressed_histories
                .insert(session_id.to_string(), vec![message.clone()]);
        }

        // 2. Persist (append single message, similar to MessageHistoryManager)
        if self.config.enable_persistence {
            self.persistence
                .append_compressed_message(session_id, &message)
                .await?;
        }

        Ok(())
    }

    /// Batch restore messages (doesn't trigger persistence, used for session restore)
    pub fn restore_session(&self, session_id: &str, messages: Vec<Message>) {
        self.compressed_histories
            .insert(session_id.to_string(), messages);
        debug!(
            "Restored session compression history: session_id={}",
            session_id
        );
    }

    /// Get copy of messages for sending to model (may be compressed)
    pub fn get_context_messages(&self, session_id: &str) -> Vec<Message> {
        self.compressed_histories
            .get(session_id)
            .map(|h| h.clone())
            .unwrap_or_default()
    }

    fn get_turn_index_to_keep(&self, turns_tokens: &[usize], token_limit: usize) -> usize {
        let mut sum = 0;
        let mut result = turns_tokens.len();
        for (idx, turn_token) in turns_tokens.iter().enumerate().rev() {
            sum += turn_token;
            if sum <= token_limit {
                result = idx;
            } else {
                break;
            }
        }
        result
    }

    /// Returns (turn_index_to_keep, turns)
    /// If turn_index_to_keep is 0, no compression is needed
    pub async fn preprocess_turns(
        &self,
        session_id: &str,
        context_window: usize,
        mut messages: Vec<Message>,
    ) -> BitFunResult<(usize, Vec<TurnWithTokens>)> {
        debug!(
            "Starting session context compression: session_id={}",
            session_id
        );

        // Remove system messages
        let message_start = {
            let mut start_idx = messages.len();
            for (idx, msg) in messages.iter().enumerate() {
                if msg.role != MessageRole::System {
                    start_idx = idx;
                    break;
                }
            }
            start_idx
        };
        let all_messages = messages.split_off(message_start);

        if all_messages.is_empty() {
            debug!(
                "Session history is empty, no compression needed: session_id={}",
                session_id
            );
            return Ok((0, Vec::new()));
        }

        let mut turns_messages = MessageHelper::group_messages_by_turns(all_messages);
        let turns_count = turns_messages.len();
        let turns_tokens: Vec<usize> = turns_messages
            .iter_mut()
            .map(|turn| turn.iter_mut().map(|m| m.get_tokens()).sum::<usize>())
            .collect();
        // Print message count and token count for each turn
        {
            let turns_msg_num: Vec<usize> = turns_messages.iter().map(|t| t.len()).collect();
            debug!(
                "Session has {} turn(s), messages per turn: {:?}, tokens per turn: {:?}",
                turns_count, turns_msg_num, turns_tokens
            );
        }

        let token_limit_keep_turns =
            (context_window as f32 * self.config.keep_turns_ratio) as usize;
        let mut turn_index_to_keep =
            self.get_turn_index_to_keep(&turns_tokens, token_limit_keep_turns);
        if turn_index_to_keep == turns_count {
            // If the last turn exceeds 30% but not 40%, keep the last turn
            let token_limit_last_turn =
                (context_window as f32 * self.config.keep_last_turn_ratio) as usize;
            if let Some(last_turn_tokens) = turns_tokens.last() {
                if *last_turn_tokens <= token_limit_last_turn {
                    turn_index_to_keep = turns_count - 1;
                }
            }
        }
        debug!("Turn index to keep: {}", turn_index_to_keep);

        let turns: Vec<TurnWithTokens> = turns_messages
            .into_iter()
            .zip(turns_tokens.into_iter())
            .map(|(msgs, tokens)| TurnWithTokens::new(msgs, tokens))
            .collect();
        Ok((turn_index_to_keep, turns))
    }

    pub async fn compress_turns(
        &self,
        session_id: &str,
        context_window: usize,
        turn_index_to_keep: usize,
        mut turns: Vec<TurnWithTokens>,
    ) -> BitFunResult<Vec<Message>> {
        if turns.is_empty() {
            debug!("No turns need compression");
            return Ok(Vec::new());
        }

        let Some(last_turn_messages) = turns.last().map(|turn| &turn.messages) else {
            debug!("No turns available after split, skipping last-turn extraction");
            return Ok(Vec::new());
        };
        let last_user_message = {
            last_turn_messages
                .first()
                .cloned()
                .and_then(|first_message| {
                    if first_message.role == MessageRole::User {
                        Some(first_message)
                    } else {
                        None
                    }
                })
        };
        let last_todo = MessageHelper::get_last_todo(&last_turn_messages);
        trace!("Last user message: {:?}", last_user_message);
        trace!("Last todo: {:?}", last_todo);
        let turns_to_keep = turns.split_off(turn_index_to_keep);

        let mut compressed_messages = Vec::new();
        if !turns.is_empty() {
            // Dynamically get Agent client for generating summary
            let ai_client_factory = get_global_ai_client_factory().await.map_err(|e| {
                BitFunError::AIClient(format!("Failed to get AI client factory: {}", e))
            })?;
            let ai_client = ai_client_factory
                .get_client_by_func_agent("compression")
                .await
                .map_err(|e| BitFunError::AIClient(format!("Failed to get AI client: {}", e)))?;

            let summary = self
                .execute_compression(ai_client, turns, context_window)
                .await?;
            trace!("Compression summary: {}", summary);

            compressed_messages.push(Message::user(format!(
                "<system-reminder>\nPrevious conversation is summarized below:\n{}\n</system-reminder>",
                summary
            )));
        }

        if !turns_to_keep.is_empty() {
            for turn in turns_to_keep {
                compressed_messages.extend(turn.messages);
            }
        } else {
            // All turns compressed, append last user message
            if let Some(last_user_message) = last_user_message {
                compressed_messages.push(last_user_message);
            }
            // Append last todo
            if let Some(last_todo) = last_todo {
                compressed_messages.push(Message::user(format!(
                    "<system-reminder>\nBelow is the most recent to-do list. Continue working on these tasks:\n{}\n</system-reminder>",
                    last_todo
                )));
            }
        }

        // Update compression history
        self.compressed_histories
            .insert(session_id.to_string(), compressed_messages.clone());

        // Persist compression history (similar to MessageHistoryManager pattern)
        if false && self.config.enable_persistence {
            if let Err(e) = self
                .persistence
                .save_compressed_messages(session_id, &compressed_messages)
                .await
            {
                warn!(
                    "Failed to persist compressed history: session_id={}, error={}",
                    session_id, e
                );
            } else {
                debug!(
                    "Compressed history persisted: session_id={}, message_count={}",
                    session_id,
                    compressed_messages.len()
                );
            }
        }

        Ok(compressed_messages)
    }

    async fn execute_compression(
        &self,
        ai_client: Arc<AIClient>,
        turns_to_compress: Vec<TurnWithTokens>,
        context_window: usize,
    ) -> BitFunResult<String> {
        debug!("Compressing {} turn(s)", turns_to_compress.len());

        fn gen_system_message_for_summary(prev_summary: &str) -> Message {
            if prev_summary.is_empty() {
                Message::system(
                    "You are a helpful AI assistant tasked with summarizing conversations."
                        .to_string(),
                )
            } else {
                Message::system(format!(
                    r#"You are a conversation summarization assistant performing an INCREMENTAL summary update.

## Previous Summary
The conversation has already been partially summarized. Here is the existing summary:

<previous_summary>
{}
</previous_summary>

## Your Task
You will be given the CONTINUATION of this conversation. Your job is to:
1. Read and understand the new conversation segment
2. MERGE the new information into the existing summary
3. Output a single, unified summary that combines both the previous summary and the new conversation

## Important Guidelines
- Preserve all important information from the previous summary
- Add new details from the current conversation segment
- If new information contradicts or updates previous information, use the newer information
- Maintain the same summary structure/format as specified in the user instructions
- The final output should be ONE cohesive summary, not two separate summaries
- Do not mention "previous summary" or "new conversation" in your output - write as if summarizing the entire conversation from the start

Be thorough and precise. Do not lose important technical details from either the previous summary or the new conversation."#,
                    prev_summary
                ))
            }
        }

        let max_tokens_in_one_request =
            (context_window as f32 * self.config.single_request_max_tokens_ratio) as usize;
        let mut current_tokens = 0;
        let mut cur_messages = Vec::new();
        let mut summary = String::new();
        let mut request_cnt = 0;
        for (idx, turn) in turns_to_compress.into_iter().enumerate() {
            if current_tokens + turn.tokens <= max_tokens_in_one_request {
                // Add current turn's messages to accumulated messages
                cur_messages.extend(turn.messages);
                current_tokens += turn.tokens;
            } else {
                // Compress accumulated messages
                if !cur_messages.is_empty() {
                    summary = self
                        .generate_summary(
                            ai_client.clone(),
                            gen_system_message_for_summary(&summary),
                            cur_messages,
                        )
                        .await?;
                    cur_messages = Vec::new(); // cur_messages has been consumed, need to reassign
                    current_tokens = 0;
                    request_cnt += 1;
                    trace!(
                        "Compression request {} completed: turn_idx={}",
                        request_cnt,
                        idx
                    );
                }

                if turn.tokens <= max_tokens_in_one_request {
                    // Add current turn's messages to accumulated messages
                    cur_messages.extend(turn.messages);
                    current_tokens = turn.tokens;
                } else {
                    // Single turn too long
                    if let Some((messages_part1, messages_part2)) =
                        MessageHelper::split_messages_in_middle(turn.messages)
                    {
                        // Compress first half and second half separately
                        summary = self
                            .generate_summary(
                                ai_client.clone(),
                                gen_system_message_for_summary(&summary),
                                messages_part1,
                            )
                            .await?;
                        request_cnt += 1;
                        debug!(
                            "[execute_compression] request_cnt={}, turn_idx={}, summary: \n{}",
                            request_cnt, idx, summary
                        );
                        summary = self
                            .generate_summary(
                                ai_client.clone(),
                                gen_system_message_for_summary(&summary),
                                messages_part2,
                            )
                            .await?;
                        request_cnt += 1;
                        debug!(
                            "[execute_compression] request_cnt={}, turn_idx={}, summary: \n{}",
                            request_cnt, idx, summary
                        );
                    } else {
                        return Err(BitFunError::Service(format!(
                            "Compression Failed, turn {} cannot be split in middle",
                            idx
                        )));
                    }
                }
            }
        }

        // Compress remaining messages
        if !cur_messages.is_empty() {
            summary = self
                .generate_summary(
                    ai_client.clone(),
                    gen_system_message_for_summary(&summary),
                    cur_messages,
                )
                .await?;
            request_cnt += 1;
            trace!("Compression request {} completed", request_cnt);
        }
        Ok(summary)
    }

    /// Generate summary for dialog turns, messages need to remove system prompt
    async fn generate_summary(
        &self,
        ai_client: Arc<AIClient>,
        system_message_for_summary: Message,
        messages: Vec<Message>,
    ) -> BitFunResult<String> {
        self.generate_summary_with_retry(ai_client, system_message_for_summary, messages, 3)
            .await
    }

    /// Generate summary for dialog turns, supports retry
    async fn generate_summary_with_retry(
        &self,
        ai_client: Arc<AIClient>,
        system_message_for_summary: Message,
        messages: Vec<Message>,
        max_tries: usize,
    ) -> BitFunResult<String> {
        let context_window = ai_client.config.context_window as usize;
        let summary_request_budget =
            (context_window as f32 * self.config.single_request_max_tokens_ratio) as usize;

        let mut summary_messages =
            self.build_summary_request_messages(system_message_for_summary, messages, summary_request_budget);
        let mut saw_context_overflow_error = false;

        let mut last_error = None;
        let base_wait_time_ms = 500;

        for attempt in 0..max_tries {
            let result = ai_client.send_message(summary_messages.clone(), None).await;

            match result {
                Ok(response) => {
                    if attempt > 0 {
                        debug!(
                            "Summary generation succeeded (attempt {}/{})",
                            attempt + 1,
                            max_tries
                        );
                    }
                    return Ok(response.text);
                }
                Err(e) => {
                    warn!(
                        "Summary generation failed (attempt {}/{}): {}",
                        attempt + 1,
                        max_tries,
                        e
                    );
                    if Self::is_context_window_exceeded_error_message(&e.to_string()) {
                        saw_context_overflow_error = true;
                        self.shrink_summary_request_messages(&mut summary_messages);
                    }
                    last_error = Some(e);

                    // If not the last attempt, wait before retrying
                    if attempt < max_tries - 1 {
                        let delay_ms = base_wait_time_ms * (1 << attempt.min(3)); // Exponential backoff
                        debug!("Waiting {}ms before retry {}...", delay_ms, attempt + 2);
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    }
                }
            }
        }

        if saw_context_overflow_error {
            warn!(
                "Summary generation exceeded context budget after {} attempts, using deterministic fallback summary",
                max_tries
            );
            return Ok(self.generate_fallback_summary(&summary_messages));
        }

        // All attempts failed
        let error_msg = format!(
            "Summary generation failed after {} attempts: {}",
            max_tries,
            last_error.unwrap_or_else(|| anyhow::anyhow!("Unknown error"))
        );
        warn!("{}", error_msg);
        Err(BitFunError::AIClient(error_msg))
    }

    fn build_summary_request_messages(
        &self,
        system_message_for_summary: Message,
        messages: Vec<Message>,
        summary_request_budget: usize,
    ) -> Vec<AIMessage> {
        let mut cleaned_messages = Self::normalize_messages_for_summary(messages);

        let mut summary_messages =
            self.compose_summary_messages(AIMessage::from(system_message_for_summary), &cleaned_messages, true);
        if Self::estimate_summary_request_tokens(&summary_messages) <= summary_request_budget {
            return summary_messages;
        }

        summary_messages = self.compose_summary_messages(
            summary_messages
                .first()
                .cloned()
                .unwrap_or_else(|| AIMessage::system("You summarize conversations.".to_string())),
            &cleaned_messages,
            false,
        );
        while Self::estimate_summary_request_tokens(&summary_messages) > summary_request_budget
            && !cleaned_messages.is_empty()
        {
            cleaned_messages.remove(0);
            let system_message = summary_messages
                .first()
                .cloned()
                .unwrap_or_else(|| AIMessage::system("You summarize conversations.".to_string()));
            summary_messages =
                self.compose_summary_messages(system_message, &cleaned_messages, false);
        }

        summary_messages
    }

    fn normalize_messages_for_summary(messages: Vec<Message>) -> Vec<AIMessage> {
        messages
            .iter()
            .filter_map(Self::normalize_single_message_for_summary)
            .collect()
    }

    fn normalize_single_message_for_summary(message: &Message) -> Option<AIMessage> {
        let content = Self::render_message_content_for_summary(message);
        if content.trim().is_empty() {
            return None;
        }

        match message.role {
            MessageRole::Assistant => Some(AIMessage::assistant(content)),
            MessageRole::System => Some(AIMessage::system(content)),
            MessageRole::User | MessageRole::Tool => Some(AIMessage::user(content)),
        }
    }

    fn render_message_content_for_summary(message: &Message) -> String {
        match &message.content {
            MessageContent::Text(text) => text.trim().to_string(),
            MessageContent::Mixed {
                reasoning_content: _,
                text,
                tool_calls,
            } => {
                if tool_calls.is_empty() {
                    return text.trim().to_string();
                }

                let tool_names = tool_calls
                    .iter()
                    .map(|call| call.tool_name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                let trimmed_text = text.trim();
                if trimmed_text.is_empty() {
                    format!("Assistant invoked tools: {}", tool_names)
                } else {
                    format!("{}\nAssistant invoked tools: {}", trimmed_text, tool_names)
                }
            }
            MessageContent::ToolResult {
                tool_name,
                result,
                result_for_assistant,
                is_error,
                ..
            } => {
                let payload = result_for_assistant
                    .as_ref()
                    .filter(|s| !s.trim().is_empty())
                    .cloned()
                    .unwrap_or_else(|| result.to_string());
                let status = if *is_error { "error" } else { "result" };
                format!("Tool {} {}:\n{}", tool_name, status, payload)
            }
        }
    }

    fn compose_summary_messages(
        &self,
        system_message: AIMessage,
        cleaned_messages: &[AIMessage],
        use_detailed_prompt: bool,
    ) -> Vec<AIMessage> {
        let mut summary_messages = vec![system_message];
        summary_messages.extend(cleaned_messages.iter().cloned());
        let prompt = if use_detailed_prompt {
            self.get_compact_prompt()
        } else {
            self.get_compact_prompt_minimal()
        };
        summary_messages.push(AIMessage::user(prompt));
        summary_messages
    }

    fn shrink_summary_request_messages(&self, summary_messages: &mut Vec<AIMessage>) {
        if summary_messages.len() > 3 {
            summary_messages.remove(1);
            return;
        }

        if let Some(last_message) = summary_messages.last_mut() {
            if let Some(content) = &mut last_message.content {
                if content.len() > 600 {
                    content.truncate(600);
                }
            }
        }
    }

    fn estimate_summary_request_tokens(summary_messages: &[AIMessage]) -> usize {
        TokenCounter::estimate_request_tokens(summary_messages, None)
    }

    fn is_context_window_exceeded_error_message(error_message: &str) -> bool {
        let msg = error_message.to_lowercase();
        msg.contains("context_length_exceeded")
            || msg.contains("exceeds the context window")
            || msg.contains("maximum context length")
            || msg.contains("prompt is too long")
    }

    fn generate_fallback_summary(&self, summary_messages: &[AIMessage]) -> String {
        let mut snippets = Vec::new();

        for message in summary_messages.iter().rev() {
            if message.role == "system" {
                continue;
            }

            let Some(content) = &message.content else {
                continue;
            };
            let trimmed = content.trim();
            if trimmed.is_empty() {
                continue;
            }

            let excerpt = if trimmed.chars().count() > 280 {
                let mut clipped = trimmed.chars().take(280).collect::<String>();
                clipped.push_str("...");
                clipped
            } else {
                trimmed.to_string()
            };

            snippets.push(format!("- {}: {}", message.role, excerpt));
            if snippets.len() >= 8 {
                break;
            }
        }

        snippets.reverse();
        if snippets.is_empty() {
            return "Previous conversation was compressed due to context limits. Continue from the latest user request and preserve recent task progress.".to_string();
        }

        format!(
            "Previous conversation was compressed due to context limits. Keep continuity using these recent highlights:\n{}",
            snippets.join("\n")
        )
    }

    /// Delete session compression history
    pub fn delete_session(&self, session_id: &str) {
        self.compressed_histories.remove(session_id);
        debug!(
            "Deleted session compression history: session_id={}",
            session_id
        );
    }

    fn get_compact_prompt(&self) -> String {
        r#"Your task is to create a detailed summary of the conversation so far, paying close attention to the user's explicit requests and your previous actions.
This summary should be thorough in capturing technical details, code patterns, and architectural decisions that would be essential for continuing development work without losing context.

Before providing your final summary, wrap your analysis in <analysis> tags to organize your thoughts and ensure you've covered all necessary points. In your analysis process:

1. Chronologically analyze each message and section of the conversation. For each section thoroughly identify:
   - The user's explicit requests and intents
   - Your approach to addressing the user's requests
   - Key decisions, technical concepts and code patterns
   - Specific details like:
     - file names
     - full code snippets
     - function signatures
     - file edits
  - Errors that you ran into and how you fixed them
  - Pay special attention to specific user feedback that you received, especially if the user told you to do something differently.
2. Double-check for technical accuracy and completeness, addressing each required element thoroughly.

Your summary should include the following sections:

1. Primary Request and Intent: Capture all of the user's explicit requests and intents in detail
2. Key Technical Concepts: List all important technical concepts, technologies, and frameworks discussed.
3. Files and Code Sections: Enumerate specific files and code sections examined, modified, or created. Pay special attention to the most recent messages and include full code snippets where applicable and include a summary of why this file read or edit is important.
4. Errors and fixes: List all errors that you ran into, and how you fixed them. Pay special attention to specific user feedback that you received, especially if the user told you to do something differently.
5. Problem Solving: Document problems solved and any ongoing troubleshooting efforts.
6. All user messages: List ALL user messages that are not tool results. These are critical for understanding the users' feedback and changing intent.
7. Pending Tasks: Outline any pending tasks that you have explicitly been asked to work on.
8. Current Work: Describe in detail precisely what was being worked on immediately before this summary request, paying special attention to the most recent messages from both user and assistant. Include file names and code snippets where applicable.
9. Optional Next Step: List the next step that you will take that is related to the most recent work you were doing. IMPORTANT: ensure that this step is DIRECTLY in line with the user's most recent explicit requests, and the task you were working on immediately before this summary request. If your last task was concluded, then only list next steps if they are explicitly in line with the users request. Do not start on tangential requests or really old requests that were already completed without confirming with the user first.
If there is a next step, include direct quotes from the most recent conversation showing exactly what task you were working on and where you left off. This should be verbatim to ensure there's no drift in task interpretation.

Here's an example of how your output should be structured:

<example>
<analysis>
[Your thought process, ensuring all points are covered thoroughly and accurately]
</analysis>

<summary>
1. Primary Request and Intent:
   [Detailed description]

2. Key Technical Concepts:
   - [Concept 1]
   - [Concept 2]
   - [...]

3. Files and Code Sections:
   - [File Name 1]
      - [Summary of why this file is important]
      - [Summary of the changes made to this file, if any]
      - [Important Code Snippet]
   - [File Name 2]
      - [Important Code Snippet]
   - [...]

4. Errors and fixes:
    - [Detailed description of error 1]:
      - [How you fixed the error]
      - [User feedback on the error if any]
    - [...]

5. Problem Solving:
   [Description of solved problems and ongoing troubleshooting]

6. All user messages: 
    - [Detailed non tool use user message]
    - [...]

7. Pending Tasks:
   - [Task 1]
   - [Task 2]
   - [...]

8. Current Work:
   [Precise description of current work]

9. Optional Next Step:
   [Optional Next step to take]

</summary>
</example>

Please provide your summary based on the conversation so far, following this structure and ensuring precision and thoroughness in your response. 
"#.to_string()
    }

    fn get_compact_prompt_minimal(&self) -> String {
        "Summarize the conversation for engineering continuation. Keep only critical facts: user requests, implemented changes, key file paths, errors and fixes, pending tasks, and the immediate next step. Output concise structured bullets."
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::CompressionManager;
    use crate::agentic::core::{Message, ToolCall, ToolResult};

    #[test]
    fn summary_normalization_eliminates_tool_protocol_messages() {
        let assistant_with_tool = Message::assistant_with_tools(
            "".to_string(),
            vec![ToolCall {
                tool_id: "call_1".to_string(),
                tool_name: "grep".to_string(),
                arguments: serde_json::json!({"pattern":"foo"}),
                is_error: false,
                should_end_turn: false,
            }],
        );
        let tool_result = Message::tool_result(ToolResult {
            tool_id: "call_1".to_string(),
            tool_name: "grep".to_string(),
            result: serde_json::json!({"matches": 1}),
            result_for_assistant: Some("found 1 match".to_string()),
            is_error: false,
            duration_ms: None,
        });

        let normalized = CompressionManager::normalize_messages_for_summary(vec![
            assistant_with_tool,
            tool_result,
        ]);

        assert_eq!(normalized.len(), 2);
        assert!(normalized.iter().all(|msg| msg.role != "tool"));
        assert!(normalized.iter().all(|msg| msg.tool_calls.is_none()));
        assert!(normalized.iter().all(|msg| msg.tool_call_id.is_none()));
    }
}
