// src/rate_limit.rs

use std::time::Duration;
use tracing::warn;

pub const DEFAULT_LLM_TIMEOUT_SECS: u64 = 35;
pub const DEFAULT_SITE_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_TG_TIMEOUT_SECS: u64 = 10;
pub const MAX_RETRIES: usize = 3;

/// Parses a standard HTTP `Retry-After` header value (either seconds or RFC 2822 date).
pub fn parse_retry_after_str(s: &str) -> Option<u64> {
    let trimmed = s.trim();
    if let Ok(secs) = trimmed.parse::<u64>() {
        return Some(secs);
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc2822(trimmed) {
        let now = chrono::Utc::now();
        let diff = dt.signed_duration_since(now).num_seconds();
        if diff > 0 {
            return Some(diff as u64);
        }
    }
    None
}

/// Parses the `retryDelay` field from Gemini's error JSON response.
/// e.g. "retryDelay": "21s" or "retryDelay": "25.5s"
pub fn parse_gemini_retry_delay(err_body: &str) -> Option<u64> {
    let val: serde_json::Value = serde_json::from_str(err_body).ok()?;
    if let Some(details) = val.get("error").and_then(|e| e.get("details")).and_then(|d| d.as_array()) {
        for item in details {
            if let Some(delay_str) = item.get("retryDelay").and_then(|d| d.as_str()) {
                let cleaned = delay_str.trim().trim_end_matches('s');
                if let Ok(secs) = cleaned.parse::<f64>() {
                    return Some(secs.ceil() as u64);
                }
            }
        }
    }
    None
}

/// Parses `retry_after` from Telegram's error JSON response.
/// e.g. {"ok": false, "error_code": 429, "parameters": {"retry_after": 5}}
pub fn parse_telegram_retry_after(err_body: &str) -> Option<u64> {
    let val: serde_json::Value = serde_json::from_str(err_body).ok()?;
    val.get("parameters")
        .and_then(|p| p.get("retry_after"))
        .and_then(|r| r.as_u64())
}

/// Helper to sleep for a duration with logging.
pub async fn sleep_with_reason(secs: u64, reason: &str) {
    warn!("{}. Sleeping for {}s timeout to respect rate limit...", reason, secs);
    tokio::time::sleep(Duration::from_secs(secs)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_retry_after_seconds() {
        assert_eq!(parse_retry_after_str("30"), Some(30));
        assert_eq!(parse_retry_after_str(" 60 \n"), Some(60));
        assert_eq!(parse_retry_after_str("invalid"), None);
    }

    #[test]
    fn test_parse_gemini_retry_delay() {
        let json = r#"{
            "error": {
                "code": 429,
                "message": "Quota exceeded",
                "details": [
                    {
                        "@type": "type.googleapis.com/google.rpc.RetryInfo",
                        "retryDelay": "21s"
                    }
                ]
            }
        }"#;
        assert_eq!(parse_gemini_retry_delay(json), Some(21));

        let json_float = r#"{
            "error": {
                "details": [
                    { "retryDelay": "12.3s" }
                ]
            }
        }"#;
        assert_eq!(parse_gemini_retry_delay(json_float), Some(13));
    }

    #[test]
    fn test_parse_telegram_retry_after() {
        let json = r#"{"ok":false,"error_code":429,"description":"Too Many Requests: retry after 8","parameters":{"retry_after":8}}"#;
        assert_eq!(parse_telegram_retry_after(json), Some(8));
    }
}
