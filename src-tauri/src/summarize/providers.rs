//! Chat clients for the cloud summarization providers (Anthropic, OpenAI, Groq).
//!
//! The HTTP call ([`complete`]) and response interpretation
//! ([`parse_response`]) are kept as separate functions on purpose, mirroring
//! `transcribe/api.rs`: parsing is a pure function over
//! `(provider, status, retry-after, body)`, so the whole error surface —
//! every status code and malformed body this module reacts to — is
//! unit-tested without a network connection (docs/CONVENTIONS.md).
//!
//! The API key is used only to build one request's auth header (`x-api-key`
//! for Anthropic, `Authorization: Bearer` for OpenAI/Groq). It is never
//! stored in a struct, never logged, and never placed in an error — no type
//! in this module holds a key, so there is nothing for a stray `Debug` or
//! `{:?}` to leak. On a successful reply the body is the model's summary of
//! the user's meeting, so a parse failure there must not echo the body back;
//! on a failure the body is the provider's own error description, which is
//! safe to surface.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{ChatMessage, ChatRole, SummarizeError, SummaryProvider};

/// How long a single chat completion request may run before it is cancelled.
/// Generous enough for a slow model on a long transcript, short enough that
/// a hung socket doesn't block the app indefinitely.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// `Retry-After` value assumed when a 429 response omits the header or sends
/// something that doesn't parse as whole seconds.
const DEFAULT_RETRY_AFTER_SECS: u64 = 30;

/// Anthropic's required API version header. Pinned, not "latest" — an
/// unannounced server-side change should not silently alter this client's
/// behavior.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// `max_tokens` sent to Anthropic (required by that API, unlike OpenAI/Groq
/// which default it server-side). A structured summary — title, ~5 bullets,
/// decisions, action items — comfortably fits well under this ceiling.
const ANTHROPIC_MAX_TOKENS: u32 = 8192;

fn endpoint(provider: SummaryProvider) -> &'static str {
    match provider {
        SummaryProvider::Anthropic => "https://api.anthropic.com/v1/messages",
        SummaryProvider::OpenAi => "https://api.openai.com/v1/chat/completions",
        SummaryProvider::Groq => "https://api.groq.com/openai/v1/chat/completions",
    }
}

/// Sends `messages` to `provider`'s chat API and returns the raw reply text.
///
/// `model` falls back to [`SummaryProvider::default_model`] when `None`.
/// `api_key` is used only to build this request's auth header; nothing in
/// this function prints the request it builds.
pub async fn complete(
    provider: SummaryProvider,
    api_key: &str,
    model: Option<&str>,
    messages: &[ChatMessage],
) -> Result<String, SummarizeError> {
    let provider_name = provider.key_name();
    let model = model.unwrap_or_else(|| provider.default_model());

    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| SummarizeError::Network {
            provider: provider_name,
            reason: "could not build the HTTP client".to_owned(),
        })?;

    let request = client.post(endpoint(provider));
    let request = match provider {
        SummaryProvider::Anthropic => request
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&AnthropicRequest::build(model, messages)),
        SummaryProvider::OpenAi | SummaryProvider::Groq => request
            .bearer_auth(api_key)
            .json(&ChatCompletionRequest::build(model, messages)),
    };

    let response = request
        .send()
        .await
        .map_err(|err| SummarizeError::Network {
            provider: provider_name,
            reason: network_failure_reason(&err),
        })?;

    let status = response.status();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response
        .text()
        .await
        .map_err(|err| SummarizeError::Network {
            provider: provider_name,
            reason: network_failure_reason(&err),
        })?;

    parse_response(provider, status, retry_after.as_deref(), &body)
}

/// Interprets one HTTP response into the raw reply text or a
/// [`SummarizeError`]. Pure: no I/O, no clock, nothing but its inputs — the
/// entire error surface of the chat clients is exercised through this
/// function in tests, with zero network access.
pub fn parse_response(
    provider: SummaryProvider,
    status: reqwest::StatusCode,
    retry_after: Option<&str>,
    body: &str,
) -> Result<String, SummarizeError> {
    let provider_name = provider.key_name();

    if status.is_success() {
        return extract_reply(provider, body).ok_or_else(|| SummarizeError::BadResponse {
            provider: provider_name,
            reason: "could not parse the chat response".to_owned(),
        });
    }

    match status.as_u16() {
        401 | 403 => Err(SummarizeError::Unauthorized {
            provider: provider_name,
        }),
        429 => Err(SummarizeError::RateLimited {
            provider: provider_name,
            retry_after_secs: parse_retry_after(retry_after),
        }),
        code => {
            let message = error_message(body);
            if let Some(message) = &message {
                if is_context_length_error(message) {
                    return Err(SummarizeError::TooLong {
                        provider: provider_name,
                        tokens: extract_token_count(message),
                    });
                }
            }
            if (500..600).contains(&code) {
                Err(SummarizeError::BadResponse {
                    provider: provider_name,
                    reason: format!("server error ({code})"),
                })
            } else {
                Err(SummarizeError::BadResponse {
                    provider: provider_name,
                    reason: message.unwrap_or_else(|| format!("unexpected status {code}")),
                })
            }
        }
    }
}

/// Parses a `Retry-After` header value as whole seconds. Anything missing or
/// unparseable falls back to [`DEFAULT_RETRY_AFTER_SECS`] — providers are not
/// required to send the header.
fn parse_retry_after(header: Option<&str>) -> u64 {
    header
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_RETRY_AFTER_SECS)
}

/// Classifies a transport-level failure without touching its `Display`
/// output, which can echo request internals.
fn network_failure_reason(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        "the request timed out".to_owned()
    } else if err.is_connect() {
        "could not connect".to_owned()
    } else if err.is_body() || err.is_decode() {
        "the response body could not be read".to_owned()
    } else if err.is_request() {
        "the request could not be built".to_owned()
    } else {
        "the request failed".to_owned()
    }
}

/// Extracts the provider's own error message, if the body matches the
/// `{"error": {"message": ...}}` shape all three providers share (Anthropic
/// wraps it in an extra top-level `"type": "error"`, but the nested object is
/// the same shape).
fn error_message(body: &str) -> Option<String> {
    serde_json::from_str::<ProviderErrorBody>(body)
        .ok()
        .map(|parsed| parsed.error.message)
}

/// Whether a provider error message describes the prompt exceeding the
/// model's context window, across the phrasings Anthropic, OpenAI and Groq
/// use for it.
fn is_context_length_error(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("context_length_exceeded")
        || lower.contains("maximum context length")
        || lower.contains("context length")
        || lower.contains("prompt is too long")
        || lower.contains("too many tokens")
}

/// Best-effort token count for a context-length error: the largest run of
/// digits in the provider's message, which in practice is the token count it
/// reports (e.g. "your messages resulted in 8945 tokens"). Falls back to 0
/// when the message names no number — providers are not required to.
fn extract_token_count(message: &str) -> usize {
    message
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|chunk| chunk.parse::<usize>().ok())
        .max()
        .unwrap_or(0)
}

/// Extracts the assistant's reply text from a successful response body.
fn extract_reply(provider: SummaryProvider, body: &str) -> Option<String> {
    match provider {
        SummaryProvider::Anthropic => {
            serde_json::from_str::<AnthropicResponse>(body)
                .ok()
                .map(|parsed| {
                    parsed
                        .content
                        .into_iter()
                        .filter(|block| block.block_type == "text")
                        .map(|block| block.text)
                        .collect::<Vec<_>>()
                        .join("")
                })
        }
        SummaryProvider::OpenAi | SummaryProvider::Groq => {
            serde_json::from_str::<ChatCompletionResponse>(body)
                .ok()
                .and_then(|parsed| parsed.choices.into_iter().next())
                .map(|choice| choice.message.content)
        }
    }
}

#[derive(Deserialize)]
struct ProviderErrorBody {
    error: ProviderErrorDetail,
}

#[derive(Deserialize)]
struct ProviderErrorDetail {
    message: String,
}

// --- Anthropic /v1/messages ---

#[derive(Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
}

#[derive(Serialize)]
struct AnthropicMessage {
    role: &'static str,
    content: String,
}

impl<'a> AnthropicRequest<'a> {
    /// Anthropic takes the system prompt as a top-level field, not a message
    /// with `role: "system"` — any `ChatRole::System` entries are pulled out
    /// of `messages` and joined into `system` instead. The split happens in
    /// [`split_system_and_conversation`], which hands back conversation turns
    /// as [`ConversationRole`] — a type with no `System` variant — so there is
    /// no system case left to map here, let alone one to panic on.
    fn build(model: &'a str, messages: &[ChatMessage]) -> Self {
        let (system, conversation) = split_system_and_conversation(messages);

        let messages = conversation
            .into_iter()
            .map(|(role, content)| AnthropicMessage {
                role: role.into(),
                content: content.to_owned(),
            })
            .collect();

        AnthropicRequest {
            model,
            max_tokens: ANTHROPIC_MAX_TOKENS,
            system,
            messages,
        }
    }
}

/// A conversation turn's role, deliberately excluding system: once messages
/// pass through [`split_system_and_conversation`], a conversation turn can
/// only ever be a user or assistant turn. Making that a type-level fact
/// (rather than an invariant upheld by filtering before mapping) is what
/// removes the `unreachable!()` this module used to carry on this path.
enum ConversationRole {
    User,
    Assistant,
}

impl From<ConversationRole> for &'static str {
    fn from(role: ConversationRole) -> Self {
        match role {
            ConversationRole::User => "user",
            ConversationRole::Assistant => "assistant",
        }
    }
}

/// Splits `messages` into Anthropic's top-level system text (joined with
/// blank lines, in case there is more than one system message) and the
/// remaining turns in their original order, each tagged with
/// [`ConversationRole`]. This is the only place `ChatRole::System` is matched
/// against; every consumer downstream works with a type that cannot express
/// it, regardless of message order.
fn split_system_and_conversation(
    messages: &[ChatMessage],
) -> (Option<String>, Vec<(ConversationRole, &str)>) {
    let mut system = Vec::new();
    let mut conversation = Vec::new();

    for message in messages {
        match message.role {
            ChatRole::System => system.push(message.content.as_str()),
            ChatRole::User => {
                conversation.push((ConversationRole::User, message.content.as_str()));
            }
            ChatRole::Assistant => {
                conversation.push((ConversationRole::Assistant, message.content.as_str()));
            }
        }
    }

    let system = if system.is_empty() {
        None
    } else {
        Some(system.join("\n\n"))
    };
    (system, conversation)
}

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContentBlock>,
}

#[derive(Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    text: String,
}

// --- OpenAI / Groq /v1/chat/completions ---

#[derive(Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: Vec<ChatCompletionMessage>,
}

#[derive(Serialize)]
struct ChatCompletionMessage {
    role: &'static str,
    content: String,
}

impl<'a> ChatCompletionRequest<'a> {
    fn build(model: &'a str, messages: &[ChatMessage]) -> Self {
        let messages = messages
            .iter()
            .map(|message| ChatCompletionMessage {
                role: match message.role {
                    ChatRole::System => "system",
                    ChatRole::User => "user",
                    ChatRole::Assistant => "assistant",
                },
                content: message.content.clone(),
            })
            .collect();
        ChatCompletionRequest { model, messages }
    }
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatCompletionChoice>,
}

#[derive(Deserialize)]
struct ChatCompletionChoice {
    message: ChatCompletionResponseMessage,
}

#[derive(Deserialize)]
struct ChatCompletionResponseMessage {
    content: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(code: u16) -> reqwest::StatusCode {
        reqwest::StatusCode::from_u16(code).expect("valid status code")
    }

    fn messages() -> Vec<ChatMessage> {
        vec![
            ChatMessage {
                role: ChatRole::System,
                content: "You are a helpful meeting summarizer.".to_owned(),
            },
            ChatMessage {
                role: ChatRole::User,
                content: "Summarize this transcript.".to_owned(),
            },
        ]
    }

    // --- endpoint ---

    #[test]
    fn each_provider_uses_its_own_endpoint() {
        assert_eq!(
            endpoint(SummaryProvider::Anthropic),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            endpoint(SummaryProvider::OpenAi),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            endpoint(SummaryProvider::Groq),
            "https://api.groq.com/openai/v1/chat/completions"
        );
    }

    // --- request building ---

    #[test]
    fn anthropic_request_pulls_the_system_message_into_a_top_level_field() {
        let request = AnthropicRequest::build("claude-sonnet-5", &messages());
        assert_eq!(
            request.system.as_deref(),
            Some("You are a helpful meeting summarizer.")
        );
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.messages[0].role, "user");
        assert_eq!(request.max_tokens, ANTHROPIC_MAX_TOKENS);
    }

    #[test]
    fn anthropic_request_with_no_system_message_omits_the_field() {
        let only_user = vec![ChatMessage {
            role: ChatRole::User,
            content: "hi".to_owned(),
        }];
        let request = AnthropicRequest::build("claude-sonnet-5", &only_user);
        assert!(request.system.is_none());
    }

    #[test]
    fn anthropic_request_serializes_without_a_system_field_when_absent() {
        let only_user = vec![ChatMessage {
            role: ChatRole::User,
            content: "hi".to_owned(),
        }];
        let request = AnthropicRequest::build("claude-sonnet-5", &only_user);
        let json = serde_json::to_value(&request).expect("serialize");
        assert!(json.get("system").is_none());
    }

    #[test]
    fn anthropic_request_pulls_out_a_system_message_wherever_it_appears() {
        // This is the shape that used to reach the removed `unreachable!()`:
        // a system message is not necessarily first or filtered out before
        // the role-mapping step runs, so it must be handled correctly no
        // matter where it sits among user/assistant turns.
        let interleaved = vec![
            ChatMessage {
                role: ChatRole::User,
                content: "hi".to_owned(),
            },
            ChatMessage {
                role: ChatRole::System,
                content: "Be concise.".to_owned(),
            },
            ChatMessage {
                role: ChatRole::Assistant,
                content: "ok".to_owned(),
            },
            ChatMessage {
                role: ChatRole::System,
                content: "Use bullet points.".to_owned(),
            },
        ];
        let request = AnthropicRequest::build("claude-sonnet-5", &interleaved);

        assert_eq!(
            request.system.as_deref(),
            Some("Be concise.\n\nUse bullet points.")
        );
        assert_eq!(request.messages.len(), 2);
        assert_eq!(request.messages[0].role, "user");
        assert_eq!(request.messages[0].content, "hi");
        assert_eq!(request.messages[1].role, "assistant");
        assert_eq!(request.messages[1].content, "ok");
    }

    #[test]
    fn chat_completion_request_keeps_the_system_role_as_a_message() {
        let request = ChatCompletionRequest::build("llama-3.3-70b-versatile", &messages());
        assert_eq!(request.messages.len(), 2);
        assert_eq!(request.messages[0].role, "system");
        assert_eq!(request.messages[1].role, "user");
    }

    // --- parse_response: success ---

    #[test]
    fn an_anthropic_success_body_returns_the_concatenated_text_blocks() {
        let body = r#"{
            "content": [
                {"type": "text", "text": "Hello "},
                {"type": "text", "text": "world"}
            ]
        }"#;
        let reply = parse_response(SummaryProvider::Anthropic, status(200), None, body)
            .expect("a 200 with a valid body parses");
        assert_eq!(reply, "Hello world");
    }

    #[test]
    fn an_anthropic_success_body_ignores_non_text_blocks() {
        let body = r#"{
            "content": [
                {"type": "thinking"},
                {"type": "text", "text": "the summary"}
            ]
        }"#;
        let reply = parse_response(SummaryProvider::Anthropic, status(200), None, body)
            .expect("non-text blocks must not break parsing");
        assert_eq!(reply, "the summary");
    }

    #[test]
    fn an_openai_success_body_returns_the_first_choice_content() {
        let body = r#"{"choices": [{"message": {"role": "assistant", "content": "the summary"}}]}"#;
        let reply = parse_response(SummaryProvider::OpenAi, status(200), None, body)
            .expect("a 200 with a valid body parses");
        assert_eq!(reply, "the summary");
    }

    #[test]
    fn a_groq_success_body_returns_the_first_choice_content() {
        let body = r#"{"choices": [{"message": {"role": "assistant", "content": "the summary"}}]}"#;
        let reply = parse_response(SummaryProvider::Groq, status(200), None, body)
            .expect("a 200 with a valid body parses");
        assert_eq!(reply, "the summary");
    }

    #[test]
    fn a_malformed_success_body_is_bad_response_and_never_echoes_the_body() {
        let body = "this meeting summary text is not valid json {{{";
        let error = parse_response(SummaryProvider::Anthropic, status(200), None, body)
            .expect_err("malformed JSON on a 200 must fail");
        match error {
            SummarizeError::BadResponse { reason, .. } => {
                assert!(!reason.contains("this meeting summary text"));
            }
            other => panic!("expected BadResponse, got {other:?}"),
        }
    }

    // --- parse_response: status mapping ---

    #[test]
    fn a_401_is_unauthorized() {
        let error = parse_response(SummaryProvider::Anthropic, status(401), None, "")
            .expect_err("401 must fail");
        assert!(
            matches!(error, SummarizeError::Unauthorized { provider } if provider == "anthropic")
        );
    }

    #[test]
    fn a_403_is_also_unauthorized() {
        let error = parse_response(SummaryProvider::OpenAi, status(403), None, "")
            .expect_err("403 must fail");
        assert!(matches!(error, SummarizeError::Unauthorized { provider } if provider == "openai"));
    }

    #[test]
    fn a_429_with_a_retry_after_header_carries_it_through() {
        let error = parse_response(SummaryProvider::Groq, status(429), Some("12"), "")
            .expect_err("429 must fail");
        assert!(matches!(
            error,
            SummarizeError::RateLimited {
                retry_after_secs: 12,
                ..
            }
        ));
    }

    #[test]
    fn a_429_without_a_retry_after_header_uses_the_default() {
        let error = parse_response(SummaryProvider::Groq, status(429), None, "")
            .expect_err("429 must fail");
        assert!(matches!(
            error,
            SummarizeError::RateLimited { retry_after_secs, .. }
                if retry_after_secs == DEFAULT_RETRY_AFTER_SECS
        ));
    }

    #[test]
    fn a_429_with_an_unparseable_retry_after_header_uses_the_default() {
        let error = parse_response(SummaryProvider::Groq, status(429), Some("soon"), "")
            .expect_err("429 must fail");
        assert!(matches!(
            error,
            SummarizeError::RateLimited { retry_after_secs, .. }
                if retry_after_secs == DEFAULT_RETRY_AFTER_SECS
        ));
    }

    #[test]
    fn a_500_is_bad_response() {
        let error = parse_response(SummaryProvider::Anthropic, status(500), None, "")
            .expect_err("500 must fail");
        assert!(matches!(error, SummarizeError::BadResponse { .. }));
    }

    #[test]
    fn a_503_is_also_bad_response() {
        let error = parse_response(SummaryProvider::OpenAi, status(503), None, "")
            .expect_err("503 must fail");
        assert!(matches!(error, SummarizeError::BadResponse { .. }));
    }

    #[test]
    fn a_400_with_a_provider_error_message_surfaces_that_message() {
        let body = r#"{"error": {"message": "invalid request", "type": "invalid_request_error"}}"#;
        let error = parse_response(SummaryProvider::Groq, status(400), None, body)
            .expect_err("400 must fail");
        match error {
            SummarizeError::BadResponse { reason, .. } => {
                assert_eq!(reason, "invalid request");
            }
            other => panic!("expected BadResponse, got {other:?}"),
        }
    }

    #[test]
    fn a_400_with_an_unparseable_body_gets_a_generic_reason() {
        let error = parse_response(SummaryProvider::Groq, status(400), None, "not json")
            .expect_err("400 must fail");
        match error {
            SummarizeError::BadResponse { reason, .. } => {
                assert_eq!(reason, "unexpected status 400");
            }
            other => panic!("expected BadResponse, got {other:?}"),
        }
    }

    #[test]
    fn an_openai_context_length_error_maps_to_too_long_with_the_token_count() {
        let body = r#"{"error": {"message": "This model's maximum context length is 8192 tokens. However, your messages resulted in 8945 tokens.", "type": "invalid_request_error", "code": "context_length_exceeded"}}"#;
        let error = parse_response(SummaryProvider::OpenAi, status(400), None, body)
            .expect_err("a context-length error must fail");
        match error {
            SummarizeError::TooLong { provider, tokens } => {
                assert_eq!(provider, "openai");
                assert_eq!(tokens, 8945);
            }
            other => panic!("expected TooLong, got {other:?}"),
        }
    }

    #[test]
    fn an_anthropic_context_length_error_maps_to_too_long() {
        let body = r#"{"type": "error", "error": {"type": "invalid_request_error", "message": "prompt is too long: 205000 tokens > 200000 maximum"}}"#;
        let error = parse_response(SummaryProvider::Anthropic, status(400), None, body)
            .expect_err("a context-length error must fail");
        assert!(matches!(
            error,
            SummarizeError::TooLong { provider, .. } if provider == "anthropic"
        ));
    }

    #[test]
    fn a_context_length_error_with_no_number_falls_back_to_zero_tokens() {
        let body = r#"{"error": {"message": "context length exceeded"}}"#;
        let error = parse_response(SummaryProvider::Groq, status(400), None, body)
            .expect_err("a context-length error must fail");
        assert!(matches!(error, SummarizeError::TooLong { tokens: 0, .. }));
    }

    #[test]
    fn errors_never_carry_the_provider_name_as_anything_but_the_key_name() {
        let error = parse_response(SummaryProvider::Anthropic, status(401), None, "")
            .expect_err("401 must fail");
        assert_eq!(error.to_string(), "anthropic rejected the key");
    }
}
