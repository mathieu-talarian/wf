//! OpenAI Chat Completions client via `async-openai`. One short completion
//! call; errors surface as 502 (`ai-request-failed`) through `AppError`.

use async_openai::config::OpenAIConfig;
use async_openai::types::chat::{
    ChatCompletionRequestSystemMessage, ChatCompletionRequestUserMessage,
    CreateChatCompletionRequestArgs,
};
use async_openai::Client;

use crate::error::AppError;
use crate::state::AppState;

/// Drafts and briefs are short, latency-sensitive completions — mini-class.
const MODEL: &str = "gpt-5-mini";
const MAX_COMPLETION_TOKENS: u32 = 600;

/// Returns the API key or the contract's 503 (`ai-unconfigured`).
pub fn require_key(state: &AppState) -> Result<String, AppError> {
    state.config.openai_api_key.clone().ok_or_else(AppError::ai_unconfigured)
}

/// One system+user completion; returns the first choice's text.
pub async fn complete(
    state: &AppState,
    system: &str,
    user_prompt: &str,
) -> Result<String, AppError> {
    let key = require_key(state)?;
    let client = Client::with_config(OpenAIConfig::new().with_api_key(key));
    let request = CreateChatCompletionRequestArgs::default()
        .model(MODEL)
        .max_completion_tokens(MAX_COMPLETION_TOKENS)
        .messages([
            ChatCompletionRequestSystemMessage::from(system).into(),
            ChatCompletionRequestUserMessage::from(user_prompt).into(),
        ])
        .build()
        .map_err(|e| AppError::ai_api(format!("OpenAI request build failed: {e}")))?;
    let response = client
        .chat()
        .create(request)
        .await
        .map_err(|e| AppError::ai_api(format!("OpenAI request failed: {e}")))?;
    response
        .choices
        .into_iter()
        .next()
        .and_then(|choice| choice.message.content)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| AppError::ai_api("OpenAI response had no text".to_string()))
}
