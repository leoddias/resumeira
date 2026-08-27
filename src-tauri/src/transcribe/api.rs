//! Cloud Whisper clients (Groq, OpenAI).
//!
//! Both providers speak the same OpenAI-compatible `/audio/transcriptions`
//! endpoint. The HTTP call ([`transcribe`]) and response interpretation
//! ([`parse_response`]) are kept as separate functions on purpose: parsing is
//! a pure function over `(provider, status, retry-after, body)`, so the whole
//! error surface — every status code and malformed body this module reacts
//! to — is unit-tested without a network connection (docs/CONVENTIONS.md).
//!
//! Nothing in this module logs or echoes the API key or the transcript text.
//! The key lives only in the `Authorization` header of one request; error
//! paths never quote a request or response body that could carry it or the
//! meeting's content back into an error message.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use super::{ApiProvider, Engine, Segment, TranscribeError, Transcript};

/// Provider file-size limit, in bytes. Chunking a longer recording is
/// out of scope for this packet (docs/TASKS.md T-M2-1).
const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;

/// `Retry-After` value assumed when a 429 response omits the header or sends
/// something that doesn't parse as whole seconds.
const DEFAULT_RETRY_AFTER_SECS: u64 = 30;

/// How long a single transcription request may run before it is cancelled.
/// Long enough for a full meeting upload on a slow connection, short enough
/// that a hung socket doesn't block the app indefinitely.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

fn provider_config(provider: ApiProvider) -> (&'static str, &'static str) {
    match provider {
        ApiProvider::Groq => ("https://api.groq.com/openai/v1", "whisper-large-v3"),
        ApiProvider::OpenAi => ("https://api.openai.com/v1", "whisper-1"),
    }
}

/// Sends `audio_path` to `provider`'s Whisper endpoint and returns the
/// resulting [`Transcript`].
///
/// `api_key` is used only to build the request's `Authorization` header; it
/// is never logged, never placed in an error, and nothing in this function
/// prints the request it builds.
pub async fn transcribe(
    provider: ApiProvider,
    api_key: &str,
    audio_path: &Path,
    language: Option<&str>,
) -> Result<Transcript, TranscribeError> {
    let provider_name = provider.key_name();

    let metadata = tokio::fs::metadata(audio_path)
        .await
        .map_err(|source| TranscribeError::Io {
            path: audio_path.display().to_string(),
            source,
        })?;
    enforce_size_limit(provider, metadata.len())?;

    let bytes = tokio::fs::read(audio_path)
        .await
        .map_err(|source| TranscribeError::Io {
            path: audio_path.display().to_string(),
            source,
        })?;

    let (base_url, model) = provider_config(provider);
    let file_name = audio_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("audio.ogg")
        .to_owned();

    let mut form = reqwest::multipart::Form::new()
        .part(
            "file",
            reqwest::multipart::Part::bytes(bytes).file_name(file_name),
        )
        .text("model", model)
        .text("response_format", "verbose_json");
    if let Some(lang) = language {
        form = form.text("language", lang.to_owned());
    }

    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| TranscribeError::Network {
            provider: provider_name,
            reason: "could not build the HTTP client".to_owned(),
        })?;

    let response = client
        .post(format!("{base_url}/audio/transcriptions"))
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await
        .map_err(|err| TranscribeError::Network {
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
        .map_err(|err| TranscribeError::Network {
            provider: provider_name,
            reason: network_failure_reason(&err),
        })?;

    parse_response(provider, status, retry_after.as_deref(), &body)
}

/// Rejects an audio file larger than the provider's upload limit before any
/// request is built. Pure and independent of I/O so it is unit-testable
/// without writing a multi-megabyte fixture to disk.
fn enforce_size_limit(provider: ApiProvider, byte_len: u64) -> Result<(), TranscribeError> {
    if byte_len > MAX_FILE_BYTES {
        Err(TranscribeError::BadResponse {
            provider: provider.key_name(),
            reason: format!(
                "audio file is {:.1} MB, over the provider's 25 MB limit",
                byte_len as f64 / (1024.0 * 1024.0)
            ),
        })
    } else {
        Ok(())
    }
}

/// Classifies a transport-level failure without touching its `Display`
/// output, which can echo request internals. Coarse-grained on purpose: it
/// never risks surfacing anything that came from the request we built.
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

/// Interprets one HTTP response into a [`Transcript`] or a
/// [`TranscribeError`]. Pure: no I/O, no clock, nothing but its inputs — the
/// entire error surface of the cloud clients is exercised through this
/// function in tests, with zero network access.
///
/// On success the body itself is the transcript, so a parse failure there
/// must not quote the body back in the error (it could contain the meeting's
/// speech). On failure the body is the provider's own error description, not
/// user content, so its `message` field is safe to surface.
pub fn parse_response(
    provider: ApiProvider,
    status: reqwest::StatusCode,
    retry_after: Option<&str>,
    body: &str,
) -> Result<Transcript, TranscribeError> {
    let provider_name = provider.key_name();

    if status.is_success() {
        return serde_json::from_str::<ApiTranscriptResponse>(body)
            .map(|parsed| parsed.into_transcript())
            .map_err(|_| TranscribeError::BadResponse {
                provider: provider_name,
                reason: "could not parse the transcription response".to_owned(),
            });
    }

    match status.as_u16() {
        401 | 403 => Err(TranscribeError::Unauthorized {
            provider: provider_name,
        }),
        429 => Err(TranscribeError::RateLimited {
            provider: provider_name,
            retry_after_secs: parse_retry_after(retry_after),
        }),
        code if (500..600).contains(&code) => Err(TranscribeError::BadResponse {
            provider: provider_name,
            reason: format!("server error ({code})"),
        }),
        code => Err(TranscribeError::BadResponse {
            provider: provider_name,
            reason: error_reason(code, body),
        }),
    }
}

/// Parses a `Retry-After` header value as whole seconds. Anything missing or
/// unparseable falls back to [`DEFAULT_RETRY_AFTER_SECS`] — providers are not
/// required to send the header, and this module never blocks retry logic on
/// its presence.
fn parse_retry_after(header: Option<&str>) -> u64 {
    header
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_RETRY_AFTER_SECS)
}

/// Extracts a human-readable reason from a non-5xx error body, if the
/// provider sent one in its usual `{"error": {"message": ...}}` shape.
fn error_reason(code: u16, body: &str) -> String {
    serde_json::from_str::<ProviderErrorBody>(body)
        .map(|parsed| parsed.error.message)
        .unwrap_or_else(|_| format!("unexpected status {code}"))
}

#[derive(Deserialize)]
struct ProviderErrorBody {
    error: ProviderErrorDetail,
}

#[derive(Deserialize)]
struct ProviderErrorDetail {
    message: String,
}

#[derive(Deserialize)]
struct ApiTranscriptResponse {
    text: String,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    segments: Vec<ApiSegment>,
}

#[derive(Deserialize)]
struct ApiSegment {
    start: f64,
    end: f64,
    text: String,
}

impl ApiTranscriptResponse {
    fn into_transcript(self) -> Transcript {
        let segments = if self.segments.is_empty() {
            // Some verbose_json responses omit `segments` for very short
            // audio; fall back to one segment spanning the whole reply so
            // the transcript still has something to summarize.
            vec![Segment {
                start: 0.0,
                end: 0.0,
                text: self.text,
                track: None,
                speaker: None,
            }]
        } else {
            self.segments
                .into_iter()
                .map(|segment| Segment {
                    start: segment.start,
                    end: segment.end,
                    text: segment.text,
                    track: None,
                    speaker: None,
                })
                .collect()
        };

        Transcript {
            // The API path has no audio in hand to second-guess a segment
            // with, so the run collapse is its only hallucination guard.
            segments: super::collapse_repeated_segments(segments),
            language: self.language,
            engine: Engine::Api,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(code: u16) -> reqwest::StatusCode {
        reqwest::StatusCode::from_u16(code).expect("valid status code")
    }

    // --- provider_config ---

    #[test]
    fn groq_uses_its_own_base_url_and_model() {
        assert_eq!(
            provider_config(ApiProvider::Groq),
            ("https://api.groq.com/openai/v1", "whisper-large-v3")
        );
    }

    #[test]
    fn openai_uses_its_own_base_url_and_model() {
        assert_eq!(
            provider_config(ApiProvider::OpenAi),
            ("https://api.openai.com/v1", "whisper-1")
        );
    }

    // --- enforce_size_limit ---

    #[test]
    fn a_file_at_the_limit_is_allowed() {
        assert!(enforce_size_limit(ApiProvider::Groq, MAX_FILE_BYTES).is_ok());
    }

    #[test]
    fn a_file_over_the_limit_names_it_in_the_error() {
        let error = enforce_size_limit(ApiProvider::OpenAi, MAX_FILE_BYTES + 1)
            .expect_err("must reject an oversized file");
        match error {
            TranscribeError::BadResponse { provider, reason } => {
                assert_eq!(provider, "openai");
                assert!(reason.contains("25 MB"), "reason was: {reason}");
            }
            other => panic!("expected BadResponse, got {other:?}"),
        }
    }

    // --- parse_response: success ---

    #[test]
    fn a_verbose_json_body_becomes_a_transcript_with_segments() {
        let body = r#"{
            "text": "hello there",
            "language": "en",
            "segments": [
                {"start": 0.0, "end": 1.2, "text": "hello"},
                {"start": 1.2, "end": 2.5, "text": " there"}
            ]
        }"#;
        let transcript = parse_response(ApiProvider::Groq, status(200), None, body)
            .expect("a 200 with a valid body parses");
        assert_eq!(transcript.engine, Engine::Api);
        assert_eq!(transcript.language.as_deref(), Some("en"));
        assert_eq!(transcript.segments.len(), 2);
        assert_eq!(transcript.segments[0].text, "hello");
        assert_eq!(transcript.segments[1].end, 2.5);
        assert!(transcript.segments.iter().all(|s| s.track.is_none()));
    }

    #[test]
    fn a_body_with_no_segments_falls_back_to_one_segment_of_the_full_text() {
        let body = r#"{"text": "just this", "language": null}"#;
        let transcript = parse_response(ApiProvider::OpenAi, status(200), None, body)
            .expect("missing segments still parses");
        assert_eq!(transcript.segments.len(), 1);
        assert_eq!(transcript.segments[0].text, "just this");
        assert!(transcript.language.is_none());
    }

    #[test]
    fn a_malformed_success_body_is_bad_response_and_never_echoes_the_body() {
        let body = "this transcript text is not valid json {{{";
        let error = parse_response(ApiProvider::Groq, status(200), None, body)
            .expect_err("malformed JSON on a 200 must fail");
        match error {
            TranscribeError::BadResponse { reason, .. } => {
                assert!(!reason.contains("this transcript text"));
            }
            other => panic!("expected BadResponse, got {other:?}"),
        }
    }

    // --- parse_response: status mapping ---

    #[test]
    fn a_401_is_unauthorized() {
        let error =
            parse_response(ApiProvider::Groq, status(401), None, "").expect_err("401 must fail");
        assert!(matches!(error, TranscribeError::Unauthorized { provider } if provider == "groq"));
    }

    #[test]
    fn a_403_is_also_unauthorized() {
        let error =
            parse_response(ApiProvider::OpenAi, status(403), None, "").expect_err("403 must fail");
        assert!(
            matches!(error, TranscribeError::Unauthorized { provider } if provider == "openai")
        );
    }

    #[test]
    fn a_429_with_a_retry_after_header_carries_it_through() {
        let error = parse_response(ApiProvider::Groq, status(429), Some("12"), "")
            .expect_err("429 must fail");
        assert!(matches!(
            error,
            TranscribeError::RateLimited {
                retry_after_secs: 12,
                ..
            }
        ));
    }

    #[test]
    fn a_429_without_a_retry_after_header_uses_the_default() {
        let error =
            parse_response(ApiProvider::Groq, status(429), None, "").expect_err("429 must fail");
        assert!(matches!(
            error,
            TranscribeError::RateLimited { retry_after_secs, .. }
                if retry_after_secs == DEFAULT_RETRY_AFTER_SECS
        ));
    }

    #[test]
    fn a_429_with_an_unparseable_retry_after_header_uses_the_default() {
        let error = parse_response(ApiProvider::Groq, status(429), Some("soon"), "")
            .expect_err("429 must fail");
        assert!(matches!(
            error,
            TranscribeError::RateLimited { retry_after_secs, .. }
                if retry_after_secs == DEFAULT_RETRY_AFTER_SECS
        ));
    }

    #[test]
    fn a_500_is_bad_response() {
        let error =
            parse_response(ApiProvider::Groq, status(500), None, "").expect_err("500 must fail");
        assert!(matches!(error, TranscribeError::BadResponse { .. }));
    }

    #[test]
    fn a_503_is_also_bad_response() {
        let error =
            parse_response(ApiProvider::OpenAi, status(503), None, "").expect_err("503 must fail");
        assert!(matches!(error, TranscribeError::BadResponse { .. }));
    }

    #[test]
    fn a_400_with_a_provider_error_message_surfaces_that_message() {
        let body =
            r#"{"error": {"message": "invalid file format", "type": "invalid_request_error"}}"#;
        let error =
            parse_response(ApiProvider::Groq, status(400), None, body).expect_err("400 must fail");
        match error {
            TranscribeError::BadResponse { reason, .. } => {
                assert_eq!(reason, "invalid file format");
            }
            other => panic!("expected BadResponse, got {other:?}"),
        }
    }

    #[test]
    fn a_400_with_an_unparseable_body_gets_a_generic_reason() {
        let error = parse_response(ApiProvider::Groq, status(400), None, "not json")
            .expect_err("400 must fail");
        match error {
            TranscribeError::BadResponse { reason, .. } => {
                assert_eq!(reason, "unexpected status 400");
            }
            other => panic!("expected BadResponse, got {other:?}"),
        }
    }

    #[test]
    fn errors_never_carry_the_provider_name_as_anything_but_the_key_name() {
        // Regression guard: the error must use `key_name()`, never a
        // display-formatted provider or anything derived from the request.
        let error =
            parse_response(ApiProvider::Groq, status(401), None, "").expect_err("401 must fail");
        assert_eq!(error.to_string(), "groq rejected the key");
    }
}
