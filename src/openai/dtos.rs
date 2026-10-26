//! OpenAI Chat Completions wire types (snake_case on the wire, like OpenAI).

use serde::{Deserialize, Serialize};
use serde_json::Value;

// -------------------------------------------------------------------- request

#[derive(Deserialize, Debug)]
#[allow(dead_code)] // accepted from the OpenAI API but unused (single user)
pub struct ChatCompletionRequest {
    pub model: String,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(rename = "top_p", default)]
    pub top_p: Option<f64>,
    #[serde(rename = "max_tokens", default)]
    pub max_tokens: Option<i64>,
    #[serde(rename = "max_completion_tokens", default)]
    pub max_completion_tokens: Option<i64>,
    #[serde(default)]
    pub stop: Option<Vec<String>>,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(rename = "stream_options", default)]
    pub stream_options: Option<StreamOptions>,
    #[serde(default)]
    pub tools: Option<Vec<OpenAiTool>>,
    #[serde(rename = "tool_choice", default)]
    pub tool_choice: Option<Value>,
    #[serde(rename = "response_format", default)]
    pub response_format: Option<ResponseFormat>,
    #[serde(default)]
    pub n: Option<i64>,
    #[serde(default)]
    pub user: Option<String>,
}

/// `content` is either a plain string or an array of typed parts.
#[derive(Deserialize, Debug)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: Option<Value>,
    #[serde(rename = "tool_calls", default)]
    pub tool_calls: Option<Vec<OpenAiToolCall>>,
    #[serde(rename = "tool_call_id", default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct OpenAiTool {
    #[serde(rename = "type")]
    pub tool_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<OpenAiFunctionDef>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct OpenAiFunctionDef {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct OpenAiToolCall {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index: Option<i64>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub call_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<OpenAiToolCallFunction>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct OpenAiToolCallFunction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct ResponseFormat {
    #[serde(rename = "type")]
    pub format_type: Option<String>,
    #[serde(rename = "json_schema", default)]
    pub json_schema: Option<JsonSchemaFormat>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)] // wire field kept for fidelity with the OpenAI API
pub struct JsonSchemaFormat {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub schema: Option<Value>,
}

#[derive(Deserialize, Debug)]
pub struct StreamOptions {
    #[serde(rename = "include_usage", default)]
    pub include_usage: Option<bool>,
}

impl ChatCompletionRequest {
    pub fn include_usage(&self) -> bool {
        self.stream_options.as_ref().and_then(|o| o.include_usage).unwrap_or(false)
    }
}

// ------------------------------------------------------------------- response

#[derive(Serialize, Debug)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: &'static str,
    pub created: i64,
    pub model: String,
    pub choices: Vec<Choice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

impl Default for ChatCompletionResponse {
    fn default() -> Self {
        Self {
            id: String::new(),
            object: "chat.completion",
            created: 0,
            model: String::new(),
            choices: Vec::new(),
            usage: None,
        }
    }
}

#[derive(Serialize, Debug)]
pub struct Choice {
    pub index: i64,
    pub message: AssistantMessage,
    #[serde(rename = "finish_reason")]
    pub finish_reason: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct AssistantMessage {
    pub role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(rename = "tool_calls", skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OpenAiToolCall>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Usage {
    #[serde(rename = "prompt_tokens")]
    pub prompt_tokens: i64,
    #[serde(rename = "completion_tokens")]
    pub completion_tokens: i64,
    #[serde(rename = "total_tokens")]
    pub total_tokens: i64,
}

// ------------------------------------------------------------------- streaming

#[derive(Serialize, Debug)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: &'static str,
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

#[derive(Serialize, Debug)]
pub struct ChunkChoice {
    pub index: i64,
    pub delta: Delta,
    #[serde(rename = "finish_reason", skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Serialize, Debug, Default)]
pub struct Delta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(rename = "tool_calls", skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OpenAiToolCall>>,
}

// --------------------------------------------------------------------- models

#[derive(Serialize, Debug)]
pub struct ModelListResponse {
    pub object: &'static str,
    pub data: Vec<ModelInfo>,
}

#[derive(Serialize, Debug)]
pub struct ModelInfo {
    pub id: String,
    pub object: &'static str,
    pub created: i64,
    pub owned_by: &'static str,
}

// ------------------------------------------------------------ account status

#[derive(Serialize, Debug)]
pub struct AccountStatusResponse {
    pub email: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(rename = "projectId", skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    pub onboarded: bool,
    #[serde(rename = "googleQuota", skip_serializing_if = "Option::is_none")]
    pub google_quota: Option<Vec<QuotaBucket>>,
}

#[derive(Serialize, Debug)]
pub struct QuotaBucket {
    #[serde(rename = "tokenType", skip_serializing_if = "Option::is_none")]
    pub token_type: Option<String>,
    #[serde(rename = "modelId", skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(rename = "remainingAmount", skip_serializing_if = "Option::is_none")]
    pub remaining_amount: Option<String>,
    #[serde(rename = "remainingFraction", skip_serializing_if = "Option::is_none")]
    pub remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime", skip_serializing_if = "Option::is_none")]
    pub reset_time: Option<String>,
}
