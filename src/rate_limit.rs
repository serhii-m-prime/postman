// src/rate_limit.rs

use std::time::Duration;
use tracing::warn;
use chrono::{DateTime, Utc};

pub const DEFAULT_LLM_TIMEOUT_SECS: u64 = 35;
pub const DEFAULT_SITE_TIMEOUT_SECS: u64 = 30;
pub const DEFAULT_TG_TIMEOUT_SECS: u64 = 10;
pub const MAX_RETRIES: usize = 3;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum GeminiErrorType {
    RpmLimit,       // 15 minutes latch
    TpmLimit,       // 15 minutes latch
    RpdLimit,       // Next day + 15 minutes latch
    HighDemand503,  // 15 minutes latch
    Other,
}

impl GeminiErrorType {
    pub fn cooldown_until(&self) -> Option<DateTime<Utc>> {
        let now = Utc::now();
        match self {
            GeminiErrorType::RpmLimit | GeminiErrorType::TpmLimit | GeminiErrorType::HighDemand503 => {
                Some(now + chrono::Duration::minutes(15))
            }
            GeminiErrorType::RpdLimit => {
                Some(calculate_next_rpd_reset())
            }
            GeminiErrorType::Other => None,
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            GeminiErrorType::RpmLimit => "RPM limit reached (15m latch)",
            GeminiErrorType::TpmLimit => "TPM limit reached (15m latch)",
            GeminiErrorType::RpdLimit => "RPD daily limit reached (latched until next day + 15m)",
            GeminiErrorType::HighDemand503 => "503 High demand / Service unavailable (15m latch)",
            GeminiErrorType::Other => "Other error",
        }
    }
}

/// Classifies Gemini error based on HTTP status code and response body.
pub fn classify_gemini_error(status_code: u16, body: &str) -> GeminiErrorType {
    if status_code == 503 {
        return GeminiErrorType::HighDemand503;
    }
    let lower = body.to_lowercase();
    if status_code == 429 {
        if lower.contains("perday") || lower.contains("per day") || lower.contains("daily") {
            return GeminiErrorType::RpdLimit;
        }
        if lower.contains("tokensperminute") || (lower.contains("token") && lower.contains("minute")) {
            return GeminiErrorType::TpmLimit;
        }
        if lower.contains("requestsperminute") || lower.contains("per minute") || lower.contains("perminute") {
            return GeminiErrorType::RpmLimit;
        }
        return GeminiErrorType::RpmLimit;
    }
    if lower.contains("unavailable") || lower.contains("high demand") {
        return GeminiErrorType::HighDemand503;
    }
    GeminiErrorType::Other
}

/// Calculates the next RPD reset time (Midnight Pacific Time / 08:00 UTC) + 15 minutes.
pub fn calculate_next_rpd_reset() -> DateTime<Utc> {
    let now = Utc::now();
    let today_reset = now.date_naive().and_hms_opt(8, 0, 0).unwrap().and_utc();
    let reset = if now < today_reset {
        today_reset
    } else {
        today_reset + chrono::Duration::days(1)
    };
    reset + chrono::Duration::minutes(15)
}

/// Formats the Gemini API endpoint URL using a template and model name.
/// Supports `{model}` or `{}` placeholder, or replaces any model name in `/models/<name>:`.
pub fn format_gemini_url(template: &str, model: &str) -> String {
    if template.contains("{model}") {
        template.replace("{model}", model)
    } else if template.contains("{}") {
        template.replace("{}", model)
    } else if let Some(start_idx) = template.find("/models/") {
        let prefix = &template[..start_idx + "/models/".len()];
        let rest = &template[start_idx + "/models/".len()..];
        if let Some(colon_idx) = rest.find(':') {
            let suffix = &rest[colon_idx..];
            format!("{}{}{}", prefix, model, suffix)
        } else {
            format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent", model)
        }
    } else {
        format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent", model)
    }
}

/// Constructs the full Gemini API endpoint URL for a given model.
pub fn get_gemini_url(model: &str) -> String {
    format_gemini_url("https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent", model)
}

/// Parses a standard HTTP `Retry-After` header value (either seconds or RFC 2822 date).
pub fn parse_retry_after_str(s: &str) -> Option<u64> {
    let trimmed = s.trim();
    if let Ok(secs) = trimmed.parse::<u64>() {
        return Some(secs);
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc2822(trimmed) {
        let now = Utc::now();
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
    fn test_classify_503_high_demand() {
        let body = r#"{
          "error": {
            "code": 503,
            "message": "This model is currently experiencing high demand. Spikes in demand are usually temporary. Please try again later.",
            "status": "UNAVAILABLE"
          }
        }"#;
        assert_eq!(classify_gemini_error(503, body), GeminiErrorType::HighDemand503);
        assert_eq!(classify_gemini_error(500, body), GeminiErrorType::HighDemand503);
    }

    #[test]
    fn test_classify_429_rpm() {
        let body = r#"{
          "error": {
            "code": 429,
            "message": "Resource has been exhausted (e.g. check quota).",
            "details": [
              {
                "@type": "type.googleapis.com/google.rpc.QuotaFailure",
                "violations": [
                  {
                    "description": "Quota exceeded for quota metric 'GenerateRequestsPerMinutePerProjectPerModel-FreeTier'"
                  }
                ]
              }
            ]
          }
        }"#;
        assert_eq!(classify_gemini_error(429, body), GeminiErrorType::RpmLimit);
    }

    #[test]
    fn test_classify_429_tpm() {
        let body = r#"{
          "error": {
            "code": 429,
            "message": "Quota exceeded for quota metric 'TokensPerMinutePerProjectPerModel-FreeTier'"
          }
        }"#;
        assert_eq!(classify_gemini_error(429, body), GeminiErrorType::TpmLimit);
    }

    #[test]
    fn test_classify_429_rpd() {
        let body = r#"{
          "error": {
            "code": 429,
            "message": "Quota exceeded for quota metric 'GenerateRequestsPerDayPerProjectPerModel-FreeTier'"
          }
        }"#;
        assert_eq!(classify_gemini_error(429, body), GeminiErrorType::RpdLimit);
    }

    #[test]
    fn test_cooldown_durations() {
        let now = Utc::now();
        let until_503 = GeminiErrorType::HighDemand503.cooldown_until().unwrap();
        assert!(until_503 >= now + chrono::Duration::minutes(14));

        let until_rpd = GeminiErrorType::RpdLimit.cooldown_until().unwrap();
        assert!(until_rpd > now + chrono::Duration::hours(1));
    }

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
    }

    #[test]
    fn test_parse_telegram_retry_after() {
        let json = r#"{"ok":false,"error_code":429,"description":"Too Many Requests: retry after 8","parameters":{"retry_after":8}}"#;
        assert_eq!(parse_telegram_retry_after(json), Some(8));
    }

    #[test]
    fn test_format_gemini_url() {
        // Template with {model}
        let t1 = "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent";
        assert_eq!(
            format_gemini_url(t1, "gemini-3.8-flash"),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.8-flash:generateContent"
        );

        // Template with {}
        let t2 = "https://my-proxy.internal/v1beta/models/{}:generateContent";
        assert_eq!(
            format_gemini_url(t2, "gemini-3.7-flash"),
            "https://my-proxy.internal/v1beta/models/gemini-3.7-flash:generateContent"
        );

        // Legacy static URL with a concrete model name
        let t3 = "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.1-flash-lite:generateContent";
        assert_eq!(
            format_gemini_url(t3, "gemini-3.8-flash"),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.8-flash:generateContent"
        );
    }
}
