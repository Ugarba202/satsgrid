use axum::{
    body::{Body, Bytes},
    extract::FromRequest,
    http::{header, Request, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Standard OpenAI-compatible error response structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAiErrorResponse {
    pub error: OpenAiErrorDetail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAiErrorDetail {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
    pub param: Option<String>,
    pub code: Option<String>,
}

impl OpenAiErrorResponse {
    pub fn new(
        message: impl Into<String>,
        error_type: impl Into<String>,
        param: Option<impl Into<String>>,
        code: Option<impl Into<String>>,
    ) -> Self {
        Self {
            error: OpenAiErrorDetail {
                message: message.into(),
                error_type: error_type.into(),
                param: param.map(Into::into),
                code: code.map(Into::into),
            },
        }
    }

    pub fn invalid_request(
        message: impl Into<String>,
        param: Option<impl Into<String>>,
        code: Option<impl Into<String>>,
    ) -> Self {
        Self::new(message, "invalid_request_error", param, code)
    }

    pub fn to_response(&self, status: StatusCode) -> Response {
        (status, Json(self)).into_response()
    }
}

impl IntoResponse for OpenAiErrorResponse {
    fn into_response(self) -> Response {
        self.to_response(StatusCode::BAD_REQUEST)
    }
}

/// Extractor that enforces strict maximum body size streaming limits and OpenAI-compatible errors.
///
/// Ensures:
/// 1. If Content-Length header is present and exceeds max_bytes, reject immediately.
/// 2. Stream chunk bytes with early termination as soon as accumulated bytes exceed max_bytes.
/// 3. Returns HTTP 413 Payload Too Large with OpenAI JSON error on overflow.
/// 4. Returns HTTP 400 Bad Request with OpenAI JSON error on malformed JSON.
#[derive(Debug, Clone)]
pub struct BoundedJson<T>(pub T);

impl<T> FromRequest<crate::node::api::AppState> for BoundedJson<T>
where
    T: serde::de::DeserializeOwned,
{
    type Rejection = Response;

    async fn from_request(
        req: Request<Body>,
        state: &crate::node::api::AppState,
    ) -> Result<Self, Self::Rejection> {
        let max_bytes: usize = state.config.server.max_request_body_bytes;

        let (parts, body) = req.into_parts();

        // 1. Check Content-Length if present as early rejection
        if let Some(content_length_str) = parts.headers.get(header::CONTENT_LENGTH) {
            if let Ok(cl_str) = content_length_str.to_str() {
                if let Ok(cl) = cl_str.parse::<usize>() {
                    if cl > max_bytes {
                        let err = OpenAiErrorResponse::invalid_request(
                            format!(
                                "Request body size exceeds maximum allowed size of {} bytes",
                                max_bytes
                            ),
                            None::<String>,
                            Some("request_too_large"),
                        );
                        return Err(err.to_response(StatusCode::PAYLOAD_TOO_LARGE));
                    }
                }
            }
        }

        // 2. Stream body chunks with strict bounding
        let bytes = match read_bounded_body(body, max_bytes).await {
            Ok(b) => b,
            Err(err_resp) => return Err(err_resp),
        };

        // 3. Deserialize JSON
        match serde_json::from_slice::<T>(&bytes) {
            Ok(val) => Ok(BoundedJson(val)),
            Err(e) => {
                let err = OpenAiErrorResponse::invalid_request(
                    format!("Invalid JSON in request body: {}", e),
                    None::<String>,
                    Some("invalid_json"),
                );
                Err(err.to_response(StatusCode::BAD_REQUEST))
            }
        }
    }
}

/// Reads the body stream up to `max_bytes`. If exceeded, terminates immediately.
pub async fn read_bounded_body(body: Body, max_bytes: usize) -> Result<Bytes, Response> {
    use futures_util::StreamExt;
    let mut stream = body.into_data_stream();
    let mut accumulated = Vec::new();

    while let Some(chunk_result) = stream.next().await {
        let chunk = match chunk_result {
            Ok(c) => c,
            Err(e) => {
                let err = OpenAiErrorResponse::invalid_request(
                    format!("Failed to read request body stream: {}", e),
                    None::<String>,
                    Some("stream_read_error"),
                );
                return Err(err.to_response(StatusCode::BAD_REQUEST));
            }
        };

        if accumulated.len().saturating_add(chunk.len()) > max_bytes {
            let err = OpenAiErrorResponse::invalid_request(
                format!(
                    "Request body size exceeds maximum allowed size of {} bytes",
                    max_bytes
                ),
                None::<String>,
                Some("request_too_large"),
            );
            return Err(err.to_response(StatusCode::PAYLOAD_TOO_LARGE));
        }

        accumulated.extend_from_slice(&chunk);
    }

    Ok(Bytes::from(accumulated))
}

/// Validated Chat Completion Request
#[derive(Debug, Clone)]
pub struct ValidatedChatRequest {
    pub raw: Value,
    pub model: String,
    pub prompt_tokens: usize,
    pub max_tokens: Option<usize>,
    pub is_stream: bool,
}

#[derive(Debug, Clone)]
pub struct ChatValidationRules {
    pub max_completion_tokens_limit: usize,
}

impl Default for ChatValidationRules {
    fn default() -> Self {
        Self {
            max_completion_tokens_limit: 128_000,
        }
    }
}

impl ValidatedChatRequest {
    /// Validates the raw JSON payload against OpenAI schema and constraints.
    pub fn validate(
        payload: Value,
        rules: &ChatValidationRules,
    ) -> Result<Self, OpenAiErrorResponse> {
        let obj = payload.as_object().ok_or_else(|| {
            OpenAiErrorResponse::invalid_request(
                "Request payload must be a JSON object",
                None::<String>,
                Some("invalid_payload_type"),
            )
        })?;

        // 1. Validate 'model'
        let model_val = obj.get("model").ok_or_else(|| {
            OpenAiErrorResponse::invalid_request(
                "Missing required field 'model'",
                Some("model"),
                Some("missing_required_field"),
            )
        })?;

        let model = model_val.as_str().ok_or_else(|| {
            OpenAiErrorResponse::invalid_request(
                "Field 'model' must be a string",
                Some("model"),
                Some("invalid_type"),
            )
        })?;

        let trimmed_model = model.trim();
        if trimmed_model.is_empty() {
            return Err(OpenAiErrorResponse::invalid_request(
                "Field 'model' must not be empty",
                Some("model"),
                Some("empty_model"),
            ));
        }

        if trimmed_model.len() > 128 {
            return Err(OpenAiErrorResponse::invalid_request(
                "Field 'model' exceeds maximum length of 128 characters",
                Some("model"),
                Some("model_too_long"),
            ));
        }

        // Validate model identifier characters (alphanumeric, dot, colon, hyphen, slash, underscore)
        if !trimmed_model.chars().all(|c| {
            c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' || c == '/' || c == ':'
        }) {
            return Err(OpenAiErrorResponse::invalid_request(
                "Field 'model' contains invalid characters. Only alphanumeric characters, '.', '-', '_', '/', and ':' are allowed.",
                Some("model"),
                Some("invalid_model_format"),
            ));
        }

        // 2. Validate 'messages'
        let messages_val = obj.get("messages").ok_or_else(|| {
            OpenAiErrorResponse::invalid_request(
                "Missing required field 'messages'",
                Some("messages"),
                Some("missing_required_field"),
            )
        })?;

        let messages_arr = messages_val.as_array().ok_or_else(|| {
            OpenAiErrorResponse::invalid_request(
                "Field 'messages' must be a JSON array",
                Some("messages"),
                Some("invalid_type"),
            )
        })?;

        if messages_arr.is_empty() {
            return Err(OpenAiErrorResponse::invalid_request(
                "Field 'messages' must contain at least one message",
                Some("messages"),
                Some("empty_messages"),
            ));
        }

        if messages_arr.len() > 2048 {
            return Err(OpenAiErrorResponse::invalid_request(
                "Field 'messages' exceeds maximum allowed message count of 2048",
                Some("messages"),
                Some("messages_limit_exceeded"),
            ));
        }

        let mut prompt_tokens = 0;

        for (idx, msg_val) in messages_arr.iter().enumerate() {
            let msg_obj = msg_val.as_object().ok_or_else(|| {
                OpenAiErrorResponse::invalid_request(
                    format!("Message at index {} must be a JSON object", idx),
                    Some(format!("messages[{}]", idx)),
                    Some("invalid_message_type"),
                )
            })?;

            // Role check
            let role_val = msg_obj.get("role").ok_or_else(|| {
                OpenAiErrorResponse::invalid_request(
                    format!("Message at index {} missing required field 'role'", idx),
                    Some(format!("messages[{}].role", idx)),
                    Some("missing_role"),
                )
            })?;

            let role = role_val.as_str().ok_or_else(|| {
                OpenAiErrorResponse::invalid_request(
                    format!("Field 'role' at index {} must be a string", idx),
                    Some(format!("messages[{}].role", idx)),
                    Some("invalid_role_type"),
                )
            })?;

            match role {
                "system" | "user" | "assistant" | "tool" | "function" => {}
                _ => {
                    return Err(OpenAiErrorResponse::invalid_request(
                        format!(
                            "Invalid role '{}' at index {}. Supported roles are: 'system', 'user', 'assistant', 'tool', 'function'",
                            role, idx
                        ),
                        Some(format!("messages[{}].role", idx)),
                        Some("invalid_role"),
                    ));
                }
            }

            // Role "tool" requires "tool_call_id"
            if role == "tool" {
                match msg_obj.get("tool_call_id") {
                    Some(id_val) => {
                        let id_str = id_val.as_str().unwrap_or("").trim();
                        if id_str.is_empty() {
                            return Err(OpenAiErrorResponse::invalid_request(
                                format!(
                                    "Tool message at index {} requires a non-empty 'tool_call_id'",
                                    idx
                                ),
                                Some(format!("messages[{}].tool_call_id", idx)),
                                Some("missing_tool_call_id"),
                            ));
                        }
                    }
                    None => {
                        return Err(OpenAiErrorResponse::invalid_request(
                            format!(
                                "Tool message at index {} missing required field 'tool_call_id'",
                                idx
                            ),
                            Some(format!("messages[{}].tool_call_id", idx)),
                            Some("missing_tool_call_id"),
                        ));
                    }
                }
            }

            // Content check
            if let Some(content_val) = msg_obj.get("content") {
                if content_val.is_null() {
                    // Assistant messages can have null content if tool_calls or function_call is present
                    let has_tool_calls = msg_obj
                        .get("tool_calls")
                        .and_then(|t| t.as_array())
                        .is_some();
                    let has_function_call = msg_obj
                        .get("function_call")
                        .and_then(|f| f.as_object())
                        .is_some();

                    if role != "assistant" || (!has_tool_calls && !has_function_call) {
                        return Err(OpenAiErrorResponse::invalid_request(
                            format!("Message content at index {} cannot be null unless assistant provides tool_calls", idx),
                            Some(format!("messages[{}].content", idx)),
                            Some("null_content"),
                        ));
                    }
                } else if let Some(content_str) = content_val.as_str() {
                    prompt_tokens +=
                        crate::node::pricing::PricingCalculator::estimate_tokens_from_text(
                            content_str,
                        );
                } else if let Some(content_arr) = content_val.as_array() {
                    for (part_idx, part_val) in content_arr.iter().enumerate() {
                        let part_obj = part_val.as_object().ok_or_else(|| {
                            OpenAiErrorResponse::invalid_request(
                                format!(
                                    "Content part at index {} in message {} must be an object",
                                    part_idx, idx
                                ),
                                Some(format!("messages[{}].content[{}]", idx, part_idx)),
                                Some("invalid_content_part"),
                            )
                        })?;

                        if let Some(text_val) = part_obj.get("text") {
                            if let Some(t_str) = text_val.as_str() {
                                prompt_tokens += crate::node::pricing::PricingCalculator::estimate_tokens_from_text(t_str);
                            }
                        }
                    }
                } else {
                    return Err(OpenAiErrorResponse::invalid_request(
                        format!("Message content at index {} must be a string, array of content parts, or null (for assistant tool calls)", idx),
                        Some(format!("messages[{}].content", idx)),
                        Some("invalid_content_type"),
                    ));
                }
            } else {
                // Content omitted: only allowed if assistant provides tool_calls
                let has_tool_calls = msg_obj
                    .get("tool_calls")
                    .and_then(|t| t.as_array())
                    .is_some();
                let has_function_call = msg_obj
                    .get("function_call")
                    .and_then(|f| f.as_object())
                    .is_some();
                if role != "assistant" || (!has_tool_calls && !has_function_call) {
                    return Err(OpenAiErrorResponse::invalid_request(
                        format!("Message at index {} is missing 'content'", idx),
                        Some(format!("messages[{}].content", idx)),
                        Some("missing_content"),
                    ));
                }
            }
        }

        // 3. Validate 'max_tokens' and 'max_completion_tokens'
        // Specification: max_completion_tokens takes precedence over max_tokens
        let max_tokens = parse_integer_token_bound(
            obj.get("max_completion_tokens"),
            "max_completion_tokens",
            rules.max_completion_tokens_limit,
        )?
        .or(parse_integer_token_bound(
            obj.get("max_tokens"),
            "max_tokens",
            rules.max_completion_tokens_limit,
        )?);

        // 4. Validate 'temperature' in [0.0, 2.0]
        if let Some(temp_val) = obj.get("temperature") {
            validate_float_range(temp_val, "temperature", 0.0, 2.0)?;
        }

        // 5. Validate 'top_p' in [0.0, 1.0]
        if let Some(top_p_val) = obj.get("top_p") {
            validate_float_range(top_p_val, "top_p", 0.0, 1.0)?;
        }

        // 6. Validate 'presence_penalty' in [-2.0, 2.0]
        if let Some(pres_val) = obj.get("presence_penalty") {
            validate_float_range(pres_val, "presence_penalty", -2.0, 2.0)?;
        }

        // 7. Validate 'frequency_penalty' in [-2.0, 2.0]
        if let Some(freq_val) = obj.get("frequency_penalty") {
            validate_float_range(freq_val, "frequency_penalty", -2.0, 2.0)?;
        }

        // 8. Validate 'stream'
        let is_stream = if let Some(stream_val) = obj.get("stream") {
            stream_val.as_bool().ok_or_else(|| {
                OpenAiErrorResponse::invalid_request(
                    "Field 'stream' must be a boolean",
                    Some("stream"),
                    Some("invalid_type"),
                )
            })?
        } else {
            false
        };

        let final_model = trimmed_model.to_string();

        Ok(ValidatedChatRequest {
            raw: payload,
            model: final_model,
            prompt_tokens,
            max_tokens,
            is_stream,
        })
    }
}

fn parse_integer_token_bound(
    val_opt: Option<&Value>,
    field_name: &str,
    upper_bound: usize,
) -> Result<Option<usize>, OpenAiErrorResponse> {
    let val = match val_opt {
        Some(v) => v,
        None => return Ok(None),
    };

    if val.is_null() {
        return Ok(None);
    }

    let num = val.as_i64().ok_or_else(|| {
        OpenAiErrorResponse::invalid_request(
            format!("Field '{}' must be an integer", field_name),
            Some(field_name),
            Some("invalid_type"),
        )
    })?;

    if num <= 0 {
        return Err(OpenAiErrorResponse::invalid_request(
            format!("Field '{}' must be greater than 0", field_name),
            Some(field_name),
            Some("parameter_out_of_range"),
        ));
    }

    if (num as usize) > upper_bound {
        return Err(OpenAiErrorResponse::invalid_request(
            format!(
                "Field '{}' ({}) exceeds configured maximum limit of {}",
                field_name, num, upper_bound
            ),
            Some(field_name),
            Some("parameter_out_of_range"),
        ));
    }

    Ok(Some(num as usize))
}

fn validate_float_range(
    val: &Value,
    field_name: &str,
    min: f64,
    max: f64,
) -> Result<f64, OpenAiErrorResponse> {
    if val.is_null() {
        return Err(OpenAiErrorResponse::invalid_request(
            format!("Field '{}' must not be null", field_name),
            Some(field_name),
            Some("invalid_type"),
        ));
    }

    let f = val.as_f64().ok_or_else(|| {
        OpenAiErrorResponse::invalid_request(
            format!("Field '{}' must be a number", field_name),
            Some(field_name),
            Some("invalid_type"),
        )
    })?;

    if !f.is_finite() {
        return Err(OpenAiErrorResponse::invalid_request(
            format!("Field '{}' must be a finite number", field_name),
            Some(field_name),
            Some("parameter_out_of_range"),
        ));
    }

    if f < min || f > max {
        return Err(OpenAiErrorResponse::invalid_request(
            format!(
                "Field '{}' must be between {} and {}, received {}",
                field_name, min, max, f
            ),
            Some(field_name),
            Some("parameter_out_of_range"),
        ));
    }

    Ok(f)
}
