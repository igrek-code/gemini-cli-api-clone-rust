//! DTOs mirroring gemini-cli's `packages/core/src/code_assist/{converter,types}.ts`
//! for the private Google Code Assist API (`cloudcode-pa.googleapis.com/v1internal`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ------------------------------------------------------------------- requests

#[derive(Serialize, Debug)]
pub struct CaGenerateContentRequest {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub user_prompt_id: String,
    pub request: CaVertexRequest,
}

#[derive(Serialize, Debug)]
pub struct CaVertexRequest {
    pub contents: Vec<CaContent>,
    #[serde(rename = "systemInstruction", skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<CaContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<CaTool>>,
    #[serde(rename = "toolConfig", skip_serializing_if = "Option::is_none")]
    pub tool_config: Option<CaToolConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<CaGenerationConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CaContent {
    pub role: String,
    pub parts: Vec<CaPart>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct CaPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(rename = "inlineData", skip_serializing_if = "Option::is_none")]
    pub inline_data: Option<CaInlineData>,
    #[serde(rename = "functionCall", skip_serializing_if = "Option::is_none")]
    pub function_call: Option<CaFunctionCall>,
    #[serde(rename = "functionResponse", skip_serializing_if = "Option::is_none")]
    pub function_response: Option<CaFunctionResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CaInlineData {
    pub mime_type: String,
    pub data: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CaFunctionCall {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CaFunctionResponse {
    pub name: String,
    pub response: Value,
}

#[derive(Serialize, Debug, Clone)]
pub struct CaTool {
    #[serde(rename = "functionDeclarations")]
    pub function_declarations: Vec<CaFunctionDeclaration>,
}

#[derive(Serialize, Debug, Clone)]
pub struct CaFunctionDeclaration {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

#[derive(Serialize, Debug, Clone)]
pub struct CaToolConfig {
    #[serde(rename = "functionCallingConfig")]
    pub function_calling_config: CaFunctionCallingConfig,
}

#[derive(Serialize, Debug, Clone)]
pub struct CaFunctionCallingConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(rename = "allowedFunctionNames", skip_serializing_if = "Option::is_none")]
    pub allowed_function_names: Option<Vec<String>>,
}

#[derive(Serialize, Debug, Clone)]
pub struct CaGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(rename = "topP", skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(rename = "maxOutputTokens", skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<i64>,
    #[serde(rename = "stopSequences", skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(rename = "responseMimeType", skip_serializing_if = "Option::is_none")]
    pub response_mime_type: Option<String>,
    #[serde(rename = "responseJsonSchema", skip_serializing_if = "Option::is_none")]
    pub response_json_schema: Option<Value>,
}

// ------------------------------------------------------------------ responses

#[derive(Deserialize, Debug, Clone, Default)]
pub struct CaGenerateContentResponse {
    #[serde(default)]
    pub response: Option<CaVertexResponse>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[allow(dead_code)] // wire fields kept for fidelity with the Google API
pub struct CaVertexResponse {
    #[serde(default)]
    pub candidates: Option<Vec<CaCandidate>>,
    #[serde(rename = "promptFeedback", default)]
    pub prompt_feedback: Option<CaPromptFeedback>,
    #[serde(rename = "usageMetadata", default)]
    pub usage_metadata: Option<CaUsageMetadata>,
    #[serde(rename = "modelVersion", default)]
    pub model_version: Option<String>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[allow(dead_code)] // wire fields kept for fidelity with the Google API
pub struct CaCandidate {
    #[serde(default)]
    pub content: Option<CaContent>,
    #[serde(rename = "finishReason", default)]
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub index: Option<i64>,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct CaPromptFeedback {
    #[serde(rename = "blockReason", default)]
    pub block_reason: Option<String>,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct CaUsageMetadata {
    #[serde(rename = "promptTokenCount", default)]
    pub prompt_token_count: Option<i64>,
    #[serde(rename = "candidatesTokenCount", default)]
    pub candidates_token_count: Option<i64>,
    #[serde(rename = "totalTokenCount", default)]
    pub total_token_count: Option<i64>,
}

impl CaUsageMetadata {
    pub fn prompt_tokens(&self) -> i64 {
        self.prompt_token_count.unwrap_or(0)
    }
    pub fn completion_tokens(&self) -> i64 {
        self.candidates_token_count.unwrap_or(0)
    }
    pub fn total_tokens(&self) -> i64 {
        self.total_token_count
            .unwrap_or_else(|| self.prompt_tokens() + self.completion_tokens())
    }
}

// ------------------------------------------------- loadCodeAssist / onboarding

#[derive(Serialize, Debug, Clone)]
pub struct CaClientMetadata {
    pub ide_type: String,
    pub platform: String,
    pub plugin_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duet_project: Option<String>,
}

impl Default for CaClientMetadata {
    fn default() -> Self {
        Self {
            ide_type: "IDE_UNSPECIFIED".into(),
            platform: "PLATFORM_UNSPECIFIED".into(),
            plugin_type: "GEMINI".into(),
            duet_project: None,
        }
    }
}

#[derive(Serialize, Debug)]
pub struct CaLoadCodeAssistRequest {
    #[serde(rename = "cloudaicompanionProject", skip_serializing_if = "Option::is_none")]
    pub cloudaicompanion_project: Option<String>,
    pub metadata: CaClientMetadata,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct CaLoadCodeAssistResponse {
    #[serde(rename = "currentTier", default)]
    pub current_tier: Option<CaTier>,
    #[serde(rename = "allowedTiers", default)]
    pub allowed_tiers: Option<Vec<CaTier>>,
    #[serde(rename = "ineligibleTiers", default)]
    pub ineligible_tiers: Option<Vec<CaIneligibleTier>>,
    #[serde(rename = "cloudaicompanionProject", default)]
    pub cloudaicompanion_project: Option<String>,
    #[serde(rename = "paidTier", default)]
    pub paid_tier: Option<CaTier>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[allow(dead_code)] // wire fields kept for fidelity with the Google API
pub struct CaTier {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(rename = "isDefault", default)]
    pub is_default: Option<bool>,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct CaIneligibleTier {
    #[serde(rename = "reasonCode", default)]
    pub reason_code: Option<String>,
    #[serde(rename = "reasonMessage", default)]
    pub reason_message: Option<String>,
    #[serde(rename = "validationUrl", default)]
    pub validation_url: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct CaOnboardUserRequest {
    #[serde(rename = "tierId")]
    pub tier_id: String,
    #[serde(rename = "cloudaicompanionProject", skip_serializing_if = "Option::is_none")]
    pub cloudaicompanion_project: Option<String>,
    pub metadata: CaClientMetadata,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct CaLongRunningOperation {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub done: Option<bool>,
    #[serde(default)]
    pub response: Option<CaOnboardUserResponse>,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct CaOnboardUserResponse {
    #[serde(rename = "cloudaicompanionProject", default)]
    pub cloudaicompanion_project: Option<CaOnboardProject>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[allow(dead_code)] // wire fields kept for fidelity with the Google API
pub struct CaOnboardProject {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct CaRetrieveUserQuotaRequest {
    pub project: String,
    #[serde(rename = "userAgent", skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct CaRetrieveUserQuotaResponse {
    #[serde(default)]
    pub buckets: Option<Vec<CaBucketInfo>>,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct CaBucketInfo {
    #[serde(rename = "remainingAmount", default)]
    pub remaining_amount: Option<String>,
    #[serde(rename = "remainingFraction", default)]
    pub remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime", default)]
    pub reset_time: Option<String>,
    #[serde(rename = "tokenType", default)]
    pub token_type: Option<String>,
    #[serde(rename = "modelId", default)]
    pub model_id: Option<String>,
}
