// src/commands/process.rs

use crate::AppContext;
use crate::db;
use tracing::{info, error, warn, debug};
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

        info!("Analyzing article [ID: {}]: {}", article.id, article.title);

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
            }
        }
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
    
    let response = client.post(&ctx.config.gemini_api_url)
        .header("X-goog-api-key", api_key) // <-- Ключ тепер тут
        .json(&payload)
        .send()
        .await?;

    let status = response.status();
    debug!("Gemini HTTP response status code received: {}", status);

    if !status.is_success() {
        let err_body = response.text().await?;
            
        error!("=================== GEMINI RAW ERROR RESPONSE ===================");
        error!("HTTP STATUS: {}", status);
        error!("JSON BODY:\n{}", err_body);
        error!("================================================================");

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err("Gemini API Rate Limit reached (429). Please slow down.".into());
        }
        return Err(format!("Gemini API failed with status {}. See raw body above.", status).into());
    }

    let json_resp: serde_json::Value = response.json().await?;
    
    let raw_json_text = json_resp["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .ok_or("Failed to extract text from Gemini response structure")?;

    let analysis: AiAnalysis = serde_json::from_str(raw_json_text)?;
    Ok(analysis)
}