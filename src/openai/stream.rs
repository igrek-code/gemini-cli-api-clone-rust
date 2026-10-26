//! Translates a stream of Code Assist (Gemini) SSE chunks into OpenAI
//! `chat.completion.chunk` SSE events. Port of `StreamRelay.kt` — one instance
//! per HTTP request.

use super::converter::{map_finish_reason, new_completion_id, usage_from};
use super::dtos::*;
use crate::codeassist::dtos::*;
use crate::config::now_unix;
use crate::error::AppError;
use serde_json::json;

pub struct StreamRelay {
    id: String,
    created: i64,
    /// Model name as requested by the client (echoed back, OpenAI-style).
    model: String,
    include_usage: bool,

    emitted_role: bool,
    saw_function_call: bool,
    block_reason: Option<String>,
    pending_finish: Option<String>,
    finished: bool,
    tool_calls_count: usize,
    usage: Option<Usage>,

    pub result: RelayResult,
}

#[derive(Debug, Default)]
pub struct RelayResult {
    pub finish_reason: Option<String>,
    pub content_chars: usize,
    pub tool_calls: usize,
    pub error: Option<String>,
}

impl StreamRelay {
    pub fn new(model: &str, include_usage: bool) -> Self {
        Self {
            id: new_completion_id(),
            created: now_unix(),
            model: model.to_string(),
            include_usage,
            emitted_role: false,
            saw_function_call: false,
            block_reason: None,
            pending_finish: None,
            finished: false,
            tool_calls_count: 0,
            usage: None,
            result: RelayResult::default(),
        }
    }

    /// Handles one upstream chunk, returning serialized chunk payloads.
    pub fn handle_chunk(&mut self, ca: &CaGenerateContentResponse) -> Vec<String> {
        let mut out = Vec::new();
        let Some(inner) = ca.response.as_ref() else { return out };

        if let Some(u) = inner.usage_metadata.as_ref() {
            self.usage = Some(usage_from(u));
        }
        if let Some(b) = inner.prompt_feedback.as_ref().and_then(|p| p.block_reason.clone()) {
            self.block_reason = Some(b);
        }

        let Some(candidate) = inner.candidates.as_ref().and_then(|c| c.first()) else { return out };

        let mut deltas: Vec<Delta> = Vec::new();
        let mut text_in_chunk: Option<String> = None;

        if let Some(parts) = candidate.content.as_ref().map(|c| &c.parts) {
            for part in parts {
                if let Some(fc) = &part.function_call {
                    self.saw_function_call = true;
                    self.tool_calls_count += 1;
                    let args = match &fc.args {
                        Some(a) => a.to_string(),
                        None => "{}".to_string(),
                    };
                    deltas.push(Delta {
                        tool_calls: Some(vec![OpenAiToolCall {
                            id: Some(super::converter::new_tool_call_id()),
                            index: Some(self.tool_calls_count as i64 - 1),
                            call_type: Some("function".into()),
                            function: Some(OpenAiToolCallFunction {
                                name: Some(fc.name.clone()),
                                arguments: Some(args),
                            }),
                        }]),
                        ..Default::default()
                    });
                } else if let Some(t) = &part.text {
                    if part.thought != Some(true) {
                        self.result.content_chars += t.len();
                        match &mut text_in_chunk {
                            Some(acc) => acc.push_str(t),
                            None => text_in_chunk = Some(t.clone()),
                        }
                    }
                }
            }
        }

        if let Some(text) = text_in_chunk {
            if self.emitted_role {
                deltas.insert(0, Delta { content: Some(text), ..Default::default() });
            } else {
                self.emitted_role = true;
                deltas.insert(0, Delta {
                    role: Some("assistant".into()),
                    content: Some(text),
                    ..Default::default()
                });
            }
        } else if !self.emitted_role && !deltas.is_empty() {
            self.emitted_role = true;
            deltas.insert(0, Delta { role: Some("assistant".into()), ..Default::default() });
        }

        for delta in deltas {
            out.push(self.chunk(ChunkChoice { index: 0, delta, finish_reason: None }));
        }

        if let Some(reason) = candidate.finish_reason.as_deref() {
            self.pending_finish = Some(map_finish_reason(Some(candidate), self.block_reason.as_deref()));
            let _ = reason;
        }

        out
    }

    /// Final chunks: finish_reason, optional usage, `[DONE]`.
    pub fn finish(&mut self) -> Vec<String> {
        self.finished = true;
        let effective = self
            .pending_finish
            .clone()
            .unwrap_or_else(|| {
                if self.saw_function_call {
                    "tool_calls".to_string()
                } else if self.block_reason.is_some() {
                    "content_filter".to_string()
                } else {
                    "stop".to_string()
                }
            });
        self.result.finish_reason = Some(effective.clone());

        let mut out = vec![self.chunk(ChunkChoice {
            index: 0,
            delta: if self.emitted_role { Delta::default() } else { Delta { role: Some("assistant".into()), ..Default::default() } },
            finish_reason: Some(effective),
        })];
        if self.include_usage && self.usage.is_some() {
            let usage = self.usage.clone().unwrap();
            out.push(self.serialize(&ChatCompletionChunk {
                id: self.id.clone(),
                object: "chat.completion.chunk",
                created: self.created,
                model: self.model.clone(),
                choices: vec![],
                usage: Some(usage),
            }));
        }
        out.push("[DONE]".to_string());
        out
    }

    /// Upstream failed mid-stream (headers already sent): SSE error + DONE.
    pub fn error_payload(&mut self, err: &AppError) -> Vec<String> {
        self.result.error = Some(err.message.clone());
        if self.finished {
            return vec![];
        }
        vec![
            json!({ "error": { "message": err.message, "type": err.etype, "code": err.code } }).to_string(),
            "[DONE]".to_string(),
        ]
    }

    fn chunk(&self, choice: ChunkChoice) -> String {
        self.serialize(&ChatCompletionChunk {
            id: self.id.clone(),
            object: "chat.completion.chunk",
            created: self.created,
            model: self.model.clone(),
            choices: vec![choice],
            usage: None,
        })
    }

    fn serialize(&self, chunk: &ChatCompletionChunk) -> String {
        serde_json::to_string(chunk).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ca_response(text: Option<&str>, function_call: Option<(&str, serde_json::Value)>, finish: Option<&str>) -> CaGenerateContentResponse {
        let mut parts = Vec::new();
        if let Some(t) = text {
            parts.push(CaPart { text: Some(t.to_string()), ..Default::default() });
        }
        if let Some((name, args)) = function_call {
            parts.push(CaPart {
                function_call: Some(CaFunctionCall { name: name.to_string(), args: Some(args) }),
                ..Default::default()
            });
        }
        CaGenerateContentResponse {
            response: Some(CaVertexResponse {
                candidates: Some(vec![CaCandidate {
                    content: Some(CaContent { role: "model".into(), parts }),
                    finish_reason: finish.map(String::from),
                    index: Some(0),
                }]),
                ..Default::default()
            }),
        }
    }

    #[test]
    fn first_text_chunk_carries_role() {
        let mut relay = StreamRelay::new("gemini-2.5-flash", false);
        let out = relay.handle_chunk(&ca_response(Some("Bonjour"), None, None));
        assert_eq!(out.len(), 1);
        let v: serde_json::Value = serde_json::from_str(&out[0]).unwrap();
        assert_eq!(v["choices"][0]["delta"]["role"], "assistant");
        assert_eq!(v["choices"][0]["delta"]["content"], "Bonjour");
    }

    #[test]
    fn later_text_chunk_has_no_role() {
        let mut relay = StreamRelay::new("gemini-2.5-flash", false);
        relay.handle_chunk(&ca_response(Some("a"), None, None));
        let out = relay.handle_chunk(&ca_response(Some("b"), None, None));
        let v: serde_json::Value = serde_json::from_str(&out[0]).unwrap();
        assert!(v["choices"][0]["delta"].get("role").is_none());
        assert_eq!(v["choices"][0]["delta"]["content"], "b");
    }

    #[test]
    fn function_call_chunk_then_tool_calls_finish() {
        let mut relay = StreamRelay::new("gemini-2.5-flash", false);
        let out = relay.handle_chunk(&ca_response(None, Some(("get_weather", json!({"city": "Paris"}))), Some("STOP")));
        // first emission prepends a role-only delta, then the tool call delta
        assert_eq!(out.len(), 2);
        let role: serde_json::Value = serde_json::from_str(&out[0]).unwrap();
        assert_eq!(role["choices"][0]["delta"]["role"], "assistant");
        assert!(role["choices"][0]["delta"].get("content").is_none());
        let v: serde_json::Value = serde_json::from_str(&out[1]).unwrap();
        assert_eq!(v["choices"][0]["delta"]["tool_calls"][0]["function"]["name"], "get_weather");
        assert_eq!(v["choices"][0]["delta"]["tool_calls"][0]["index"], 0);

        let done = relay.finish();
        assert_eq!(done.len(), 2); // finish chunk + [DONE]
        let v: serde_json::Value = serde_json::from_str(&done[0]).unwrap();
        assert_eq!(v["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(done[1], "[DONE]");
    }

    #[test]
    fn finish_emits_usage_when_requested() {
        let mut relay = StreamRelay::new("gemini-2.5-flash", true);
        let mut ca = ca_response(Some("hi"), None, Some("STOP"));
        ca.response.as_mut().unwrap().usage_metadata = Some(CaUsageMetadata {
            prompt_token_count: Some(10),
            candidates_token_count: Some(5),
            total_token_count: Some(15),
        });
        relay.handle_chunk(&ca);
        let done = relay.finish();
        assert_eq!(done.len(), 3); // finish + usage + [DONE]
        let v: serde_json::Value = serde_json::from_str(&done[1]).unwrap();
        assert_eq!(v["usage"]["total_tokens"], 15);
        assert!(v["choices"].as_array().unwrap().is_empty());
    }

    #[test]
    fn thought_parts_are_excluded() {
        let mut relay = StreamRelay::new("gemini-2.5-flash", false);
        let ca = CaGenerateContentResponse {
            response: Some(CaVertexResponse {
                candidates: Some(vec![CaCandidate {
                    content: Some(CaContent {
                        role: "model".into(),
                        parts: vec![CaPart {
                            text: Some("thinking…".into()),
                            thought: Some(true),
                            ..Default::default()
                        }],
                    }),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
        };
        let out = relay.handle_chunk(&ca);
        assert!(out.is_empty());
    }

    #[test]
    fn error_mid_stream() {
        let mut relay = StreamRelay::new("gemini-2.5-flash", false);
        relay.handle_chunk(&ca_response(Some("partial"), None, None));
        let out = relay.error_payload(&AppError::upstream(
            axum::http::StatusCode::BAD_GATEWAY,
            "Google stream failed",
            "google_upstream_error",
        ));
        assert_eq!(out.len(), 2);
        let v: serde_json::Value = serde_json::from_str(&out[0]).unwrap();
        assert_eq!(v["error"]["code"], "google_upstream_error");
        assert_eq!(out[1], "[DONE]");
    }

    #[test]
    fn block_reason_maps_to_content_filter() {
        let mut relay = StreamRelay::new("gemini-2.5-flash", false);
        let mut ca = ca_response(None, None, None);
        ca.response.as_mut().unwrap().prompt_feedback = Some(CaPromptFeedback {
            block_reason: Some("SAFETY".into()),
        });
        relay.handle_chunk(&ca);
        let done = relay.finish();
        let v: serde_json::Value = serde_json::from_str(&done[0]).unwrap();
        assert_eq!(v["choices"][0]["finish_reason"], "content_filter");
    }
}
