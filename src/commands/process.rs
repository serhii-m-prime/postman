// src/commands/process.rs

use crate::AppContext;
use crate::db;
use crate::rate_limit;
use std::time::Duration;
use tracing::{info, error, warn};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Deserialize, Serialize)]
pub struct AiAnalysis {
    pub score: i32,
    pub event_slug: String,
    pub reason: String,
    pub category: String
}

pub async fn run(ctx: &AppContext, target_article_id: Option<i32>) {
    info!("Starting PROCESS stage (AI Analysis)...");

    let api_key = &ctx.config.gemini_api_key;

    let mut articles = match db::get_unprocessed_articles(ctx) {
        Ok(list) => list,
        Err(e) => {
            error!("Failed to fetch unprocessed articles from DB: {}", e);
            return;
        }
    };

    if articles.is_empty() {
        info!("No new articles to process.");
        return;
    }

    match target_article_id {
        Some(target_id) => {
            articles.retain(|a| a.id == target_id);
            if articles.is_empty() {
                warn!("Article with ID {} not found or already processed.", target_id);
                return;
            }
            info!("Target mode: Processing single article with ID {}.", target_id);
        }
        None => {
            articles.truncate(1);
            info!("Batch mode: Picked the single oldest unprocessed article for test.");
        }
    }

    let client = reqwest::Client::new();

    for article in articles {
        if let Some(target_id) = target_article_id {
            if article.id != target_id { continue; }
        }

        info!("Analyzing article [ID: {} SOURCE: {}]: {}", article.id, article.feed_name, article.title);

        let article_context = format!(
            "FEED: {}\nTITLE: {}\nCONTEXT:\n{}",
            article.feed_name,
            article.title,
            article.description.unwrap_or_else(|| "No details".to_string())
        );

        match call_gemini_api(&ctx, &client, &api_key, &ctx.config.prompts.scoring, &article_context).await {
            Ok(analysis) => {
                info!("AI Category: {} | Score: {} | Slug: [{}]", analysis.category, analysis.score, analysis.event_slug);

                if let Err(e) = db::update_processed_article(
                    &ctx, 
                    article.id, 
                    analysis.score,
                    &analysis.category, 
                    &analysis.event_slug, 
                    &analysis.reason, 
                ) {
                    error!("Failed to save AI results to DB for ID {}: {}", article.id, e);
                }
            }
            Err(e) => {
                error!("Gemini API failed for article {}: {}", article.id, e);
                let err_msg = e.to_string();
                if err_msg.contains("429") || err_msg.to_lowercase().contains("rate limit") {
                    warn!("Gemini API rate limit (429) persists after retries. Pausing with 60s timeout and halting PROCESS stage to prevent server spam.");
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    break;
                }
            }
        }

        // Polite delay between articles to stay within Gemini RPM limits (e.g. 15 RPM for free tier)
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }

    info!("PROCESS stage completed.");
}

async fn call_gemini_api(
    ctx: &AppContext, 
    client: &reqwest::Client,
    api_key: &str,
    system_instruction: &str,
    content_text: &str,
) -> Result<AiAnalysis, Box<dyn std::error::Error>> {
    let payload = json!({
        "systemInstruction": {
            "parts": [{ "text": system_instruction }]
        },
        "contents": [{
            "parts": [{ "text": content_text }]
        }],
        "generationConfig": {
            "responseMimeType": "application/json",
            "responseSchema": {
                "type": "object",
                "properties": {
                    "reason": { 
                        "type": "string", 
                        "description": "Brief justification for score and category" 
                    },
                    "category": { 
                        "type": "string",
                        "enum": ["programming_ai", "biotech_med", "mechanics", "communications", "drones", "electronics", "space_energy", "other"]
                    },
                    "event_slug": {
                        "type": "string",
                        "description": "short slug indentifier of article in cebab-case"
                    },
                    "score": { 
                        "type": "integer", 
                        "description": "Innovation score from 1 to 10" 
                    }
                },
                "required": ["reason", "category", "event_slug", "score"]
            }
        }
    });

    let max_retries = rate_limit::MAX_RETRIES;
    for attempt in 0..=max_retries {
        let response = client.post(&ctx.config.gemini_api_url)
            .header("X-goog-api-key", api_key)
            .json(&payload)
            .send()
            .await?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let header_retry = response.headers().get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(rate_limit::parse_retry_after_str);

            let err_body = response.text().await.unwrap_or_default();
            let body_delay = rate_limit::parse_gemini_retry_delay(&err_body);

            let wait_secs = header_retry
                .or(body_delay)
                .unwrap_or(rate_limit::DEFAULT_LLM_TIMEOUT_SECS);

            if attempt < max_retries {
                rate_limit::sleep_with_reason(
                    wait_secs,
                    &format!("Gemini API Rate Limit reached (429) during scoring. Attempt {}/{}", attempt + 1, max_retries),
                ).await;
                continue;
            } else {
                error!("=================== GEMINI RAW ERROR RESPONSE ===================");
                error!("HTTP STATUS: {}", status);
                error!("JSON BODY:\n{}", err_body);
                error!("================================================================");
                return Err("Gemini API Rate Limit reached (429). Please slow down.".into());
            }
        }

        if !status.is_success() {
            let err_body = response.text().await?;
                
            error!("=================== GEMINI RAW ERROR RESPONSE ===================");
            error!("HTTP STATUS: {}", status);
            error!("JSON BODY:\n{}", err_body);
            error!("================================================================");

            return Err(format!("Gemini API failed with status {}. See raw body above.", status).into());
        }

        let json_resp: serde_json::Value = response.json().await?;
        
        let raw_json_text = json_resp["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .ok_or("Failed to extract text from Gemini response structure")?;

        let analysis: AiAnalysis = serde_json::from_str(raw_json_text)?;
        return Ok(analysis);
    }

    Err("Gemini API scoring failed after retries".into())
}