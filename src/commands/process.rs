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
                if err_msg.contains("exhausted") || err_msg.contains("rate-limited") || err_msg.contains("high demand") || err_msg.contains("429") || err_msg.contains("503") {
                    warn!("All Gemini models are currently latched in cooldown. Pausing with 60s timeout and halting PROCESS stage.");
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

    let configured_models = ctx.config.get_models();

    // Iterate through models from newest/best to oldest
    for model_name in &configured_models {
        // Check if model is currently latched in cooldown
        if let Ok(Some(cooldown_until)) = db::get_model_cooldown(ctx, model_name) {
            info!("Model '{}' is latched in cooldown until {}. Skipping...", model_name, cooldown_until);
            continue;
        }

        let url = ctx.config.get_gemini_url(model_name);
        info!("Calling Gemini model: {}", model_name);

        let response = match client.post(&url)
            .header("X-goog-api-key", api_key)
            .json(&payload)
            .send()
            .await 
        {
            Ok(resp) => resp,
            Err(e) => {
                error!("Network error calling Gemini model {}: {}", model_name, e);
                continue;
            }
        };

        let status = response.status();
        let status_code = status.as_u16();

        if !status.is_success() {
            let err_body = response.text().await.unwrap_or_default();
            let err_type = rate_limit::classify_gemini_error(status_code, &err_body);

            match err_type {
                rate_limit::GeminiErrorType::RpmLimit
                | rate_limit::GeminiErrorType::TpmLimit
                | rate_limit::GeminiErrorType::RpdLimit
                | rate_limit::GeminiErrorType::HighDemand503 => {
                    if let Some(until) = err_type.cooldown_until() {
                        warn!(
                            "Model '{}' encountered HTTP {} ({}). Latched until {}. Jumping to next model in chain...",
                            model_name,
                            status_code,
                            err_type.description(),
                            until
                        );
                        if let Err(e) = db::set_model_cooldown(ctx, model_name, until, err_type.description()) {
                            error!("Failed to save model cooldown in DB: {}", e);
                        }
                    }
                    continue; // Jump to next model in the fallback chain!
                }
                rate_limit::GeminiErrorType::Other => {
                    error!("=================== GEMINI RAW ERROR RESPONSE ===================");
                    error!("MODEL: {}", model_name);
                    error!("HTTP STATUS: {}", status);
                    error!("JSON BODY:\n{}", err_body);
                    error!("================================================================");
                    return Err(format!("Gemini API failed with status {}. See raw body above.", status).into());
                }
            }
        }

        let json_resp: serde_json::Value = response.json().await?;
        let raw_json_text = json_resp["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .ok_or("Failed to extract text from Gemini response structure")?;

        let analysis: AiAnalysis = serde_json::from_str(raw_json_text)?;
        return Ok(analysis);
    }

    // All models in the list are latched or unavailable
    if let Ok(Some((earliest_model, earliest_time))) = db::get_earliest_cooldown(ctx, &configured_models) {
        error!("All Gemini models are exhausted or latched. Earliest available is '{}' at {}.", earliest_model, earliest_time);
    }
    Err("All Gemini models in fallback chain are currently rate-limited, high-demand (503), or unavailable.".into())
}