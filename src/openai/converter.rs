//! Converts between the OpenAI Chat Completions format and the Code Assist
//! (Gemini/Vertex-style) format used by gemini-cli. Port of `OpenAiConverter.kt`.

use super::dtos::*;
use crate::codeassist::dtos::*;
use crate::config::Config;
use crate::error::AppError;
use crate::oauth::random_hex;
use serde_json::{json, Value};
use uuid::Uuid;

pub fn new_completion_id() -> String {
    format!("chatcmpl-{}", random_hex(12))
}

pub fn new_tool_call_id() -> String {
    format!("call_{}", random_hex(12))
}

/// Returns the Gemini model for an OpenAI-style model name, or `None` when unknown.
pub fn resolve_model(cfg: &Config, requested: &str) -> Option<String> {
    let m = requested.trim();
    if let Some(mapped) = cfg.model_aliases.get(m) {
        return Some(mapped.clone());
    }
    if let Some(stripped) = m.strip_prefix("models/") {
        return Some(stripped.to_string());
    }
    if m.starts_with("gemini-") {
        Some(m.to_string())
    } else {
        None
    }
}

pub fn to_ca_request(
    _cfg: &Config,
    req: &ChatCompletionRequest,
    model: &str,
    project: Option<&str>,
) -> Result<CaGenerateContentRequest, AppError> {
    let (system_parts, contents) = convert_messages(req)?;

    // Kotlin: one CaTool per function declaration.
    let tools: Option<Vec<CaTool>> = match req.tools.as_ref().filter(|t| !t.is_empty()) {
        Some(tools) => {
            let mut ca_tools = Vec::new();
            for tool in tools {
                match tool.tool_type.as_str() {
                    "function" => {
                        let f = tool.function.as_ref().ok_or_else(|| {
                            AppError::bad_request_param("Function tool is missing its 'function' definition.", "tools")
                        })?;
                        ca_tools.push(CaTool {
                            function_declarations: vec![CaFunctionDeclaration {
                                name: f.name.clone(),
                                description: f.description.clone(),
                                parameters: f.parameters.clone(),
                            }],
                        });
                    }
                    other => {
                        return Err(AppError::bad_request_param(
                            format!("Unsupported tool type '{other}'. Only 'function' tools are supported."),
                            "tools",
                        ))
                    }
                }
            }
            Some(ca_tools)
        }
        None => None,
    };

    let tool_config = match tools.as_ref() {
        Some(_) => Some(convert_tool_choice(req.tool_choice.as_ref())?),
        None => None,
    };

    Ok(CaGenerateContentRequest {
        model: model.to_string(),
        project: project.map(String::from),
        user_prompt_id: Uuid::new_v4().to_string(),
        request: CaVertexRequest {
            contents,
            system_instruction: system_parts
                .filter(|p| !p.is_empty())
                .map(|parts| CaContent { role: "user".into(), parts }),
            tools,
            tool_config,
            generation_config: Some(convert_generation_config(req)?),
            session_id: Some(Uuid::new_v4().to_string()),
        },
    })
}

fn convert_messages(req: &ChatCompletionRequest) -> Result<(Option<Vec<CaPart>>, Vec<CaContent>), AppError> {
    if req.messages.is_empty() {
        return Err(AppError::bad_request_param("'messages' must be a non-empty list.", "messages"));
    }
    let mut system_parts: Vec<CaPart> = Vec::new();
    let mut contents: Vec<CaContent> = Vec::new();
    // tool_call_id -> function name, to name tool responses on the way back
    let mut pending_tool_names: std::collections::HashMap<String, String> = Default::default();

    for msg in &req.messages {
        match msg.role.as_str() {
            "system" | "developer" => system_parts.extend(text_parts(msg.content.as_ref())?),

            "user" => contents.push(CaContent { role: "user".into(), parts: content_parts(msg.content.as_ref(), "user")? }),

            "assistant" => {
                let mut parts: Vec<CaPart> = Vec::new();
                parts.extend(text_parts(msg.content.as_ref())?);
                if let Some(calls) = &msg.tool_calls {
                    for tc in calls {
                        let Some(f) = &tc.function else { continue };
                        let args: Value = match f.arguments.as_deref() {
                            None | Some("") => json!({}),
                            Some(s) => serde_json::from_str(s).map_err(|_| {
                                AppError::bad_request_param(
                                    format!("tool_calls arguments for '{}' is not valid JSON.", f.name.as_deref().unwrap_or("?")),
                                    "messages",
                                )
                            })?,
                        };
                        let id = tc.id.clone().unwrap_or_else(|| format!("call_{}", random_hex(8)));
                        pending_tool_names.insert(id, f.name.clone().unwrap_or_else(|| "unknown_function".into()));
                        parts.push(CaPart {
                            function_call: Some(CaFunctionCall {
                                name: f.name.clone().unwrap_or_else(|| "unknown_function".into()),
                                args: Some(args),
                            }),
                            ..Default::default()
                        });
                    }
                }
                if !parts.is_empty() {
                    contents.push(CaContent { role: "model".into(), parts });
                }
            }

            "tool" => {
                let name = msg
                    .name
                    .clone()
                    .or_else(|| msg.tool_call_id.as_ref().and_then(|id| pending_tool_names.get(id).cloned()))
                    .or_else(|| msg.tool_call_id.clone())
                    .unwrap_or_else(|| "unknown_function".into());
                let raw = match msg.content.as_ref() {
                    None | Some(Value::Null) => String::new(),
                    Some(Value::String(s)) => s.clone(),
                    Some(v) => v.to_string(),
                };
                let response_node: Value = if raw.trim().is_empty() {
                    json!({})
                } else {
                    match serde_json::from_str::<Value>(&raw) {
                        Ok(v) => v,
                        Err(_) => json!({ "result": raw }),
                    }
                };
                let wrapped = if response_node.is_object() { response_node } else { json!({ "result": response_node }) };
                contents.push(CaContent {
                    role: "function".into(),
                    parts: vec![CaPart {
                        function_response: Some(CaFunctionResponse { name, response: wrapped }),
                        ..Default::default()
                    }],
                });
            }

            other => {
                return Err(AppError::bad_request_param(
                    format!("Invalid role '{other}' in messages (expected system, user, assistant or tool)."),
                    "messages",
                ));
            }
        }
    }

    if contents.is_empty() {
        return Err(AppError::bad_request_param("No user/assistant messages found in 'messages'.", "messages"));
    }
    let system = if system_parts.is_empty() { None } else { Some(system_parts) };
    Ok((system, contents))
}

fn text_parts(content: Option<&Value>) -> Result<Vec<CaPart>, AppError> {
    Ok(extract_parts(content, "system/developer")?.into_iter().filter(|p| p.text.is_some()).collect())
}

fn content_parts(content: Option<&Value>, role: &str) -> Result<Vec<CaPart>, AppError> {
    extract_parts(content, role)
}

fn extract_parts(content: Option<&Value>, role: &str) -> Result<Vec<CaPart>, AppError> {
    let Some(content) = content else { return Ok(vec![]) };
    match content {
        Value::Null => Ok(vec![]),
        Value::String(t) => {
            if t.is_empty() {
                Ok(vec![])
            } else {
                Ok(vec![CaPart { text: Some(t.clone()), ..Default::default() }])
            }
        }
        Value::Array(parts_in) => {
            let mut parts = Vec::new();
            for node in parts_in {
                let part_type = node.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match part_type {
                    "text" => {
                        if let Some(t) = node.get("text").and_then(|t| t.as_str()) {
                            parts.push(CaPart { text: Some(t.to_string()), ..Default::default() });
                        }
                    }
                    "image_url" => {
                        let url = node
                            .pointer("/image_url/url")
                            .and_then(|u| u.as_str())
                            .ok_or_else(|| AppError::bad_request_param("image_url part is missing 'url'.", "messages"))?;
                        let data_uri_prefix = "data:";
                        if !url.starts_with(data_uri_prefix) {
                            return Err(AppError::bad_request_param(
                                "Only base64 data URIs (data:image/png;base64,...) are supported for image_url; remote URLs are not fetched.",
                                "messages",
                            ));
                        }
                        let header_end = url[data_uri_prefix.len()..]
                            .find(',')
                            .map(|i| i + data_uri_prefix.len())
                            .ok_or_else(|| AppError::bad_request_param("Malformed data URI in image_url.", "messages"))?;
                        let mime_type = url[data_uri_prefix.len()..header_end]
                            .split(';')
                            .next()
                            .unwrap_or_default()
                            .to_string();
                        let data = url[header_end + 1..].to_string();
                        parts.push(CaPart {
                            inline_data: Some(CaInlineData { mime_type, data }),
                            ..Default::default()
                        });
                    }
                    "" => return Err(AppError::bad_request_param("Message content part is missing 'type'.", "messages")),
                    other => {
                        return Err(AppError::bad_request_param(
                            format!("Unsupported content part type '{other}'."),
                            "messages",
                        ))
                    }
                }
            }
            Ok(parts)
        }
        _ => Err(AppError::bad_request_param(
            format!("Message content must be a string or an array of parts (role: {role})."),
            "messages",
        )),
    }
}

fn convert_tool_choice(tool_choice: Option<&Value>) -> Result<CaToolConfig, AppError> {
    let mode_of = |mode: &str| CaToolConfig {
        function_calling_config: CaFunctionCallingConfig { mode: Some(mode.into()), allowed_function_names: None },
    };
    match tool_choice {
        None | Some(Value::Null) => Ok(mode_of("AUTO")),
        Some(Value::String(s)) => match s.as_str() {
            "auto" => Ok(mode_of("AUTO")),
            "none" => Ok(mode_of("NONE")),
            "required" => Ok(mode_of("ANY")),
            other => Err(AppError::bad_request_param(format!("Invalid tool_choice '{other}'."), "tool_choice")),
        },
        Some(Value::Object(o)) if o.get("type").and_then(|t| t.as_str()) == Some("function") => {
            let name = o
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .ok_or_else(|| AppError::bad_request_param("tool_choice.function.name is required.", "tool_choice"))?;
            Ok(CaToolConfig {
                function_calling_config: CaFunctionCallingConfig {
                    mode: Some("ANY".into()),
                    allowed_function_names: Some(vec![name.to_string()]),
                },
            })
        }
        _ => Err(AppError::bad_request_param("Invalid tool_choice.", "tool_choice")),
    }
}

fn convert_generation_config(req: &ChatCompletionRequest) -> Result<CaGenerationConfig, AppError> {
    let max_tokens = req.max_completion_tokens.or(req.max_tokens);
    if let Some(mt) = max_tokens {
        if mt <= 0 {
            return Err(AppError::bad_request_param("max_tokens must be a positive integer.", "max_tokens"));
        }
    }
    let (mime, schema): (Option<String>, Option<Value>) = match req.response_format.as_ref().and_then(|f| f.format_type.as_deref()) {
        None | Some("text") => (None, None),
        Some("json_object") => (Some("application/json".into()), None),
        Some("json_schema") => {
            let schema = req
                .response_format
                .as_ref()
                .and_then(|f| f.json_schema.as_ref())
                .and_then(|s| s.schema.clone())
                .ok_or_else(|| {
                    AppError::bad_request_param("response_format.json_schema.schema is required.", "response_format")
                })?;
            (Some("application/json".into()), Some(schema))
        }
        Some(other) => {
            return Err(AppError::bad_request_param(
                format!("Unsupported response_format.type '{other}'."),
                "response_format",
            ))
        }
    };
    Ok(CaGenerationConfig {
        temperature: req.temperature.map(|t| t.clamp(0.0, 2.0)),
        top_p: req.top_p.map(|t| t.clamp(0.0, 1.0)),
        max_output_tokens: max_tokens,
        stop_sequences: req
            .stop
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| s.iter().take(5).cloned().collect()),
        response_mime_type: mime,
        response_json_schema: schema,
    })
}

// --------------------------------------------------------------- response side

pub fn to_completion(
    resp: &CaGenerateContentResponse,
    id: &str,
    created: i64,
    model: &str,
) -> ChatCompletionResponse {
    let inner = resp.response.as_ref();
    let candidate = inner.and_then(|i| i.candidates.as_ref()).and_then(|c| c.first());
    let block_reason = inner.and_then(|i| i.prompt_feedback.as_ref()).and_then(|p| p.block_reason.as_deref());
    let finish_reason = map_finish_reason(candidate, block_reason);

    let mut text = String::new();
    let mut tool_calls: Vec<OpenAiToolCall> = Vec::new();

    if let Some(parts) = candidate.and_then(|c| c.content.as_ref()).map(|c| &c.parts) {
        for part in parts {
            if let Some(fc) = &part.function_call {
                tool_calls.push(OpenAiToolCall {
                    id: Some(new_tool_call_id()),
                    index: None,
                    call_type: Some("function".into()),
                    function: Some(OpenAiToolCallFunction {
                        name: Some(fc.name.clone()),
                        arguments: Some(match &fc.args {
                            Some(a) => a.to_string(),
                            None => "{}".to_string(),
                        }),
                    }),
                });
            } else if let Some(t) = &part.text {
                if part.thought != Some(true) {
                    text.push_str(t);
                }
            }
        }
    }

    ChatCompletionResponse {
        id: id.to_string(),
        object: "chat.completion",
        created,
        model: model.to_string(),
        choices: vec![Choice {
            index: 0,
            message: AssistantMessage {
                role: "assistant",
                content: if text.trim().is_empty() { None } else { Some(text) },
                tool_calls: if tool_calls.is_empty() { None } else { Some(tool_calls) },
            },
            finish_reason: Some(finish_reason),
        }],
        usage: inner.and_then(|i| i.usage_metadata.as_ref()).map(usage_from),
    }
}

pub fn map_finish_reason(candidate: Option<&CaCandidate>, block_reason: Option<&str>) -> String {
    let has_function_call = candidate
        .and_then(|c| c.content.as_ref())
        .map(|c| c.parts.iter().any(|p| p.function_call.is_some()))
        .unwrap_or(false);
    if has_function_call {
        return "tool_calls".into();
    }
    let Some(reason) = candidate.and_then(|c| c.finish_reason.as_deref()) else {
        return if block_reason.is_some() { "content_filter".into() } else { "stop".into() };
    };
    match reason {
        "STOP" => "stop".into(),
        "MAX_TOKENS" => "length".into(),
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "LANGUAGE" => "content_filter".into(),
        _ => "stop".into(),
    }
}

pub fn usage_from(m: &CaUsageMetadata) -> Usage {
    Usage {
        prompt_tokens: m.prompt_tokens(),
        completion_tokens: m.completion_tokens(),
        total_tokens: m.total_tokens(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use serde_json::json;

    fn cfg() -> Config {
        Config::from_env()
    }

    fn req(json: serde_json::Value) -> ChatCompletionRequest {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn resolve_model_aliases_and_gemini() {
        let c = cfg();
        assert_eq!(resolve_model(&c, "gpt-4o").unwrap(), "gemini-2.5-pro");
        assert_eq!(resolve_model(&c, "gpt-5-mini").unwrap(), "gemini-2.5-flash");
        assert_eq!(resolve_model(&c, "gemini-2.5-pro").unwrap(), "gemini-2.5-pro");
        assert_eq!(resolve_model(&c, "models/gemini-2.5-flash").unwrap(), "gemini-2.5-flash");
        assert!(resolve_model(&c, "claude-3").is_none());
        assert!(resolve_model(&c, "gpt-4-turbo").is_none());
    }

    #[test]
    fn system_user_assistant_tool_roundtrip() {
        let c = cfg();
        let request = req(json!({
            "model": "gpt-4o",
            "messages": [
                {"role": "system", "content": "Tu es utile."},
                {"role": "user", "content": "Météo à Paris ?"},
                {"role": "assistant", "content": null, "tool_calls": [
                    {"id": "call_1", "type": "function", "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\"}"}}
                ]},
                {"role": "tool", "tool_call_id": "call_1", "content": "{\"temp\": 21}"}
            ]
        }));
        let ca = to_ca_request(&c, &request, "gemini-2.5-pro", Some("proj-1")).unwrap();
        let r = &ca.request;
        assert_eq!(ca.model, "gemini-2.5-pro");
        assert_eq!(ca.project.as_deref(), Some("proj-1"));
        assert_eq!(
            r.system_instruction.as_ref().unwrap().parts[0].text.as_deref(),
            Some("Tu es utile.")
        );
        assert_eq!(r.contents.len(), 3); // user + model(tool call) + function(response)
        assert_eq!(r.contents[0].role, "user");
        assert_eq!(r.contents[1].role, "model");
        assert!(r.contents[1].parts[0].function_call.is_some());
        let fr = r.contents[2].parts[0].function_response.as_ref().unwrap();
        assert_eq!(fr.name, "get_weather"); // resolved from pending map
        assert_eq!(fr.response, json!({"temp": 21}));
        assert!(r.session_id.is_some());
    }

    #[test]
    fn tool_message_non_json_wraps_result() {
        let c = cfg();
        let request = req(json!({
            "model": "gemini-2.5-flash",
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "assistant", "tool_calls": [
                    {"id": "c1", "type": "function", "function": {"name": "f", "arguments": "{}"}}
                ]},
                {"role": "tool", "tool_call_id": "c1", "content": "plain text output"}
            ]
        }));
        let ca = to_ca_request(&c, &request, "gemini-2.5-flash", None).unwrap();
        let fr = ca.request.contents[2].parts[0].function_response.as_ref().unwrap();
        assert_eq!(fr.response, json!({"result": "plain text output"}));
    }

    #[test]
    fn image_url_data_uri_becomes_inline_data() {
        let c = cfg();
        let request = req(json!({
            "model": "gemini-2.5-flash",
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "Qu'est-ce que c'est ?"},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,aGVsbG8="}}
            ]}]
        }));
        let ca = to_ca_request(&c, &request, "gemini-2.5-flash", None).unwrap();
        let parts = &ca.request.contents[0].parts;
        assert_eq!(parts[0].text.as_deref(), Some("Qu'est-ce que c'est ?"));
        let inline = parts[1].inline_data.as_ref().unwrap();
        assert_eq!(inline.mime_type, "image/png");
        assert_eq!(inline.data, "aGVsbG8=");
    }

    #[test]
    fn remote_image_url_rejected() {
        let c = cfg();
        let request = req(json!({
            "model": "gemini-2.5-flash",
            "messages": [{"role": "user", "content": [
                {"type": "image_url", "image_url": {"url": "https://example.com/x.png"}}
            ]}]
        }));
        let err = to_ca_request(&c, &request, "gemini-2.5-flash", None).unwrap_err();
        assert_eq!(err.status, axum::http::StatusCode::BAD_REQUEST);
        assert!(err.message.contains("data URIs"));
    }

    #[test]
    fn tools_and_tool_choice_named_function() {
        let c = cfg();
        let request = req(json!({
            "model": "gemini-2.5-flash",
            "messages": [{"role": "user", "content": "?"}],
            "tools": [{"type": "function", "function": {"name": "f1", "parameters": {"type": "object"}}}],
            "tool_choice": {"type": "function", "function": {"name": "f1"}}
        }));
        let ca = to_ca_request(&c, &request, "gemini-2.5-flash", None).unwrap();
        let tools = ca.request.tools.as_ref().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].function_declarations[0].name, "f1");
        let tc = ca.request.tool_config.as_ref().unwrap();
        assert_eq!(tc.function_calling_config.mode.as_deref(), Some("ANY"));
        assert_eq!(
            tc.function_calling_config.allowed_function_names.as_deref(),
            Some(&["f1".to_string()][..])
        );
    }

    #[test]
    fn generation_config_clamps_and_maps() {
        let c = cfg();
        let request = req(json!({
            "model": "gemini-2.5-flash",
            "messages": [{"role": "user", "content": "x"}],
            "temperature": 5.0,
            "top_p": 2.0,
            "max_tokens": 100,
            "stop": ["a", "b"],
            "response_format": {"type": "json_object"}
        }));
        let ca = to_ca_request(&c, &request, "gemini-2.5-flash", None).unwrap();
        let g = ca.request.generation_config.as_ref().unwrap();
        assert_eq!(g.temperature, Some(2.0));
        assert_eq!(g.top_p, Some(1.0));
        assert_eq!(g.max_output_tokens, Some(100));
        assert_eq!(g.response_mime_type.as_deref(), Some("application/json"));
        assert_eq!(g.stop_sequences.as_deref(), Some(&["a".to_string(), "b".to_string()][..]));
    }

    #[test]
    fn invalid_roles_and_empty_messages_rejected() {
        let c = cfg();
        let bad_role = req(json!({
            "model": "gemini-2.5-flash",
            "messages": [{"role": "bogus", "content": "x"}]
        }));
        assert!(to_ca_request(&c, &bad_role, "m", None).is_err());

        let only_system = req(json!({
            "model": "gemini-2.5-flash",
            "messages": [{"role": "system", "content": "x"}]
        }));
        let err = to_ca_request(&c, &only_system, "m", None).unwrap_err();
        assert!(err.message.contains("No user/assistant"));
    }

    #[test]
    fn finish_reason_mapping() {
        let candidate = |reason: Option<&str>, parts: Vec<CaPart>| CaCandidate {
            content: Some(CaContent { role: "model".into(), parts }),
            finish_reason: reason.map(String::from),
            index: Some(0),
        };
        assert_eq!(map_finish_reason(Some(&candidate(Some("STOP"), vec![])), None), "stop");
        assert_eq!(map_finish_reason(Some(&candidate(Some("MAX_TOKENS"), vec![])), None), "length");
        assert_eq!(map_finish_reason(Some(&candidate(Some("SAFETY"), vec![])), None), "content_filter");
        assert_eq!(map_finish_reason(Some(&candidate(Some("OTHER"), vec![])), None), "stop");
        assert_eq!(map_finish_reason(None, Some("SAFETY")), "content_filter");
        assert_eq!(map_finish_reason(None, None), "stop");
        let fc = candidate(None, vec![CaPart {
            function_call: Some(CaFunctionCall { name: "f".into(), args: None }),
            ..Default::default()
        }]);
        assert_eq!(map_finish_reason(Some(&fc), None), "tool_calls");
    }

    #[test]
    fn to_completion_extracts_text_and_tool_calls() {
        let ca = CaGenerateContentResponse {
            response: Some(CaVertexResponse {
                candidates: Some(vec![CaCandidate {
                    content: Some(CaContent {
                        role: "model".into(),
                        parts: vec![
                            CaPart { text: Some("Il fait ".into()), ..Default::default() },
                            CaPart { text: Some("21°C".into()), ..Default::default() },
                            CaPart { text: Some("raisonnement caché".into()), thought: Some(true), ..Default::default() },
                            CaPart {
                                function_call: Some(CaFunctionCall { name: "f".into(), args: Some(json!({"a":1})) }),
                                ..Default::default()
                            },
                        ],
                    }),
                    finish_reason: Some("STOP".into()),
                    index: Some(0),
                }]),
                usage_metadata: Some(CaUsageMetadata {
                    prompt_token_count: Some(10),
                    candidates_token_count: Some(4),
                    total_token_count: None,
                }),
                ..Default::default()
            }),
        };
        let resp = to_completion(&ca, "id-1", 123, "gpt-4o");
        assert_eq!(resp.id, "id-1");
        assert_eq!(resp.model, "gpt-4o");
        let choice = &resp.choices[0];
        assert_eq!(choice.message.content.as_deref(), Some("Il fait 21°C")); // thought excluded
        assert_eq!(choice.finish_reason.as_deref(), Some("tool_calls"));
        let tc = choice.message.tool_calls.as_ref().unwrap()[0].function.as_ref().unwrap();
        assert_eq!(tc.name.as_deref(), Some("f"));
        assert_eq!(tc.arguments.as_deref(), Some("{\"a\":1}"));
        let usage = resp.usage.unwrap();
        assert_eq!(usage.total_tokens, 14); // 10 + 4
    }
}
