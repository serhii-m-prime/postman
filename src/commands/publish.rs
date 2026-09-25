use crate::AppContext;
use crate::db::{self, PublishableArticle};
use crate::rate_limit;
use std::time::Duration;
use tracing::{info, debug, error, warn};
use serde_json::{json, Value};
use scraper::{Html, Selector};
use wreq::Client;
use wreq_util::Emulation;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct EnrichmentResult {
    translated_title: String,
    emoji: String,
    summary: String,
    tags: Vec<String>,
}

pub async fn run(ctx: &AppContext, category: String) {
    info!("Starting PUBLISH process for category: {}", category);

    // Stage 1: Select the best unprocessed articles from DB

    // Initialize wreq client with Chrome emulation to bypass CAPTCHA/Cloudflare.
    // This perfectly spoofs TLS fingerprints, HTTP/2 settings, and User-Agent.
    let client = match Client::builder()
        .emulation(Emulation::Chrome137) // You can also try Firefox136 or Safari versions
        .build() 
    {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to build wreq client: {}", e);
            return;
        }
    };

    let mut published_count = 0;

    // Fetch and process ONE article at a time until we reach the target limit
    while published_count < ctx.config.articles_in_post {
        let articles = select_articles(ctx, &category).await;

        if articles.is_empty() {
            info!("No more unpublished articles available in DB. Stopping.");
            break; // Exit loop if DB is empty
        }

        let article = &articles[0]; // Take the single article
        debug!("Processing article ID: {} ({})", article.id, article.event_slug);

        let css_selector = ctx.config.feeds.iter()
            .find(|f| f.name == article.feed_name)
            .and_then(|f| f.content_selector.as_deref())
            .unwrap_or("p");

        if ctx.is_debug {
            debug!("Using selector '{}' for article from feed '{}'", css_selector, article.feed_name);
            send_debug_log(ctx, &client, &format!("Using selector '{}' for article from feed '{}' score: {}", css_selector, article.feed_name, article.score), true).await;
        }

        // Stage 2: Scraping
        let content = match scrape_content(&client, &article.link, css_selector).await {
            Ok(text) => text,
            Err(e) => {
                error!("Scraping failed for ID {}: {}", article.id, e);
                
                // 1. Update DB (mark skipped)
                if let Err(db_err) = db::mark_article_as_skipped(ctx, article.id, &article.event_slug) {
                    error!("Failed to update skipped status in DB: {}", db_err);
                }
                
                // 2. Send Debug Alert to Telegram
                let debug_msg = format!(
                    "⚠️ <b>Scraping Failed</b>\n<b>ID:</b> {}\n<b>Feed:</b> {}\n<b>Error:</b> {}\n<a href=\"{}\">Original Link</a>",
                    article.id, article.feed_name, e, article.link
                );
                send_debug_log(ctx, &client, &debug_msg, true).await;
                
                // 3. Move to the next iteration (does NOT increment published_count)
                continue;
            }
        };

        // Stage 3: Summarization
        let enriched = match summarize_article(ctx, &client, &article.title, &content).await {
            Ok(sum) => sum,
            Err(e) => {
                error!("Failed to summarize article {}: {}", article.id, e);
                let err_msg = e.to_string();
                if err_msg.contains("429") || err_msg.to_lowercase().contains("rate limit") {
                    warn!("Gemini API rate limit exceeded during summarization. Pausing with 60s timeout and halting publish pipeline to prevent server spam.");
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    break;
                } else {
                    // For non-rate-limit errors (e.g. content policy blocked, malformed text),
                    // mark the article as skipped so we don't infinitely retry the same article.
                    if let Err(db_err) = db::mark_article_as_skipped(ctx, article.id, &article.event_slug) {
                        error!("Failed to update skipped status in DB for article {}: {}", article.id, db_err);
                    }
                    continue; 
                }
            }
        };

        // Stage 4: Telegram Delivery
        let is_sent = send_to_telegram(ctx, &client, article, &enriched).await;

        // Stage 5: DB Update & Counter
        if is_sent {
            if let Err(e) = db::mark_as_published(ctx, article.id).await {
                error!("Failed to update DB status for article {}: {}", article.id, e);
            } else {
                info!("Successfully published article ID: {}", article.id);
                published_count += 1; // Increment only on complete success
            }
        } else {
            error!("Telegram delivery failed. Halting publish pipeline to avoid spam/ban.");
            break; // Stop completely if TG is rejecting requests
        }
    }

    info!("PUBLISH process completed.");
}

// --- STAGE 1: Selection ---
async fn select_articles(ctx: &AppContext, category: &str) -> Vec<PublishableArticle> {
    if ctx.is_debug {
        debug!("[DEBUG] Querying best unpublished articles for category: {}", category);
    }
    
    match db::get_best_unpublished_articles(ctx, category) {
        Ok(articles) => articles,
        Err(e) => {
            error!("Failed to fetch articles from database: {}", e);
            vec![] // Return empty vector on failure to stop the pipeline gracefully
        }
    } 
}

// --- STAGE 2: Scraping ---
async fn scrape_content(client: &wreq::Client, url: &str, selector_str: &str) -> Result<String, Box<dyn std::error::Error>> {
    let max_retries = 2;
    let mut response = None;

    for attempt in 0..=max_retries {
        let resp = client.get(url).send().await?;
        if resp.status().as_u16() == 429 {
            let retry_after = resp.headers().get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(rate_limit::parse_retry_after_str)
                .unwrap_or(rate_limit::DEFAULT_SITE_TIMEOUT_SECS);

            if attempt < max_retries {
                rate_limit::sleep_with_reason(
                    retry_after,
                    &format!("Scraping site '{}' returned 429 (Too Many Requests). Retry attempt {}/{}", url, attempt + 1, max_retries),
                ).await;
                continue;
            } else {
                rate_limit::sleep_with_reason(
                    retry_after,
                    &format!("Scraping site '{}' hit 429 after {} retries", url, max_retries),
                ).await;
                return Err(format!("HTTP request failed with status: 429 Too Many Requests for URL: {}", url).into());
            }
        }
        response = Some(resp);
        break;
    }

    let response = match response {
        Some(r) => r,
        None => return Err("Failed to obtain response while scraping".into()),
    };

    if !response.status().is_success() {
        return Err(format!("HTTP request failed with status: {}", response.status()).into());
    }

    let html = response.text().await?;

    let document = Html::parse_document(&html);
    let paragraph_selector = Selector::parse(selector_str).unwrap();
    let mut extracted_text = String::new();

    for element in document.select(&paragraph_selector) {
        let text = element.text().collect::<Vec<_>>().join(" ");
        let clean_text = text.trim();
        
        if clean_text.len() > 40 {
            extracted_text.push_str(clean_text);
            extracted_text.push('\n');
            extracted_text.push('\n');
        }
    }

    if extracted_text.is_empty() {
        return Err("Failed to extract meaningful text".into());
    }

    let max_length = 20_000;
    if extracted_text.len() > max_length {
        extracted_text.truncate(max_length);
        extracted_text.push_str("\n... [CONTENT TRUNCATED]");
    }

    Ok(extracted_text)
}

// --- STAGE 3: LLM Summarization ---
async fn summarize_article(
    ctx: &AppContext, 
    client: &wreq::Client,
    article_title: &str, 
    text: &str
) -> Result<EnrichmentResult, Box<dyn std::error::Error>> {
    // Assuming you have 'enrichment' or 'publish' prompt in your config
    let system_prompt = &ctx.config.prompts.enrichment; 

    let url = format!("{}?key={}", &ctx.config.gemini_api_url, &ctx.config.gemini_api_key);

    // Build the request structure expected by the Gemini API
    let content_text = format!("TITLE: {}\n\nCONTENT:\n{}", article_title, text);
    let payload = json!({
        "system_instruction": {
            "parts": { "text": system_prompt }
        },
        "contents": [{
            "parts": [{ "text": content_text }]
        }],
        "generationConfig": {
            "temperature": 0.3,
            "responseMimeType": "application/json",
            "responseSchema": {
                "type": "object",
                "properties": {
                    "translated_title": { 
                        "type": "string",
                        "description": "Catchy translated title in Ukrainian"
                    },
                    "emoji": { 
                        "type": "string",
                        "description": "Exactly one highly relevant emoji"
                    },
                    "summary": { 
                        "type": "string",
                        "description": "3-7 sentences summary in Ukrainian"
                    },
                    "tags": { 
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "3-5 relevant short tags in English without # symbol"
                    }
                },
                "required": ["translated_title", "emoji", "summary", "tags"]
            }
        }
    });

    let max_retries = rate_limit::MAX_RETRIES;
    for attempt in 0..=max_retries {
        let response = client.post(&url).json(&payload).send().await?;

        // Handle standard Free Tier rate limits gracefully with timeout backoff
        if response.status().as_u16() == 429 {
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
                    &format!("Gemini API Rate Limit Exceeded (429) during summarization. Attempt {}/{}", attempt + 1, max_retries),
                ).await;
                continue;
            } else {
                send_debug_log(ctx, client, "Gemini API Rate Limit Exceeded (429 Too Many Requests) after retries with timeout", true).await;
                return Err("Gemini API Rate Limit Exceeded (429 Too Many Requests)".into());
            }
        }

        if !response.status().is_success() {
            let err_body = response.text().await?;
            let error_string = format!("Gemini API returned error: {}", err_body);
            send_debug_log(ctx, client, &error_string, true).await;
            return Err(format!("Gemini API returned error: {}", err_body).into());
        }

        // Parse the JSON response
        let json_resp: Value = response.json().await?;

        // Safely extract the generated text from the deeply nested JSON structure
        let raw_json_text = json_resp["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .ok_or("Failed to extract 'text' field from Gemini response")?;

        let enrichment_result: EnrichmentResult = serde_json::from_str(raw_json_text)?;
        return Ok(enrichment_result);
    }

    Err("Gemini API summarization failed after retries".into())
}

// --- STAGE 4: Telegram Integration ---
async fn send_to_telegram(
    ctx: &AppContext, 
    client: &wreq::Client, 
    article: &PublishableArticle, 
    enriched: &EnrichmentResult
) -> bool {
    let token = &ctx.config.telegram.bot_token;
    let channel_id = if ctx.is_debug {
        &ctx.config.telegram.debug_channel_id
    } else {
        &ctx.config.telegram.main_channel_id
    };

    let tags_str = enriched.tags.iter()
        .map(|t| format!("#{}", t.replace(" ", "_").replace("-", "_")))
        .collect::<Vec<_>>()
        .join(" ");

    // Format the final message (e.g., MarkdownV2 or HTML)
    let message = format!(
        "{} <b>{}</b>\n\n{}\n\n{}\n\n<a href=\"{}\">Читати оригінал\n(score:{})</a>", 
        enriched.emoji, 
        enriched.translated_title, 
        enriched.summary, 
        tags_str,
        article.link,
        article.score
    );

    let url = format!("https://api.telegram.org/bot{}/sendMessage", token);
    
    let payload = json!({
        "chat_id": channel_id,
        "text": message,
        "parse_mode": "HTML",
        "no_webpage": true,
        "disable_web_page_preview": true,
        "invert_media": true
    });

    if ctx.is_debug {
        debug!("[DEBUG] Simulated sending to TG Channel {}:\n{}\n\n {} \n\n", channel_id, message, enriched.summary);
    }

    let max_retries = 2;
    for attempt in 0..=max_retries {
        match client.post(&url).json(&payload).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.as_u16() == 429 {
                    let err_body = resp.text().await.unwrap_or_default();
                    let retry_after = rate_limit::parse_telegram_retry_after(&err_body)
                        .unwrap_or(rate_limit::DEFAULT_TG_TIMEOUT_SECS);
                    if attempt < max_retries {
                        rate_limit::sleep_with_reason(
                            retry_after,
                            &format!("Telegram returned 429 Too Many Requests. Attempt {}/{}", attempt + 1, max_retries),
                        ).await;
                        continue;
                    } else {
                        error!("Telegram returned 429 Too Many Requests after retries.");
                        return false;
                    }
                } else if !status.is_success() {
                    let err_body = resp.text().await.unwrap_or_default();
                    error!("Failed to send message to Telegram. Status {}: {}", status, err_body);
                    return false;
                } else {
                    return true;
                }
            }
            Err(e) => {
                error!("Failed to send message to Telegram: {}", e);
                if attempt < max_retries {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    continue;
                }
                return false;
            }
        }
    }
    false
}

// --- HELPER: Telegram Debug Notification ---
async fn send_debug_log(ctx: &AppContext, client: &wreq::Client, message: &str, disable_notification: bool) {
    let token = &ctx.config.telegram.bot_token;
    // Always use debug channel for system alerts
    let channel_id = &ctx.config.telegram.debug_channel_id; 
    
    let url = format!("https://api.telegram.org/bot{}/sendMessage", token);
    let payload = json!({
        "chat_id": channel_id,
        "text": message,
        "parse_mode": "HTML",
        "no_webpage": true,
        "disable_notification": disable_notification
    });

    match client.post(&url).json(&payload).send().await {
        Ok(resp) => {
            if resp.status().as_u16() == 429 {
                warn!("Telegram returned 429 on send_debug_log. Skipping to avoid Telegram spam.");
            }
        }
        Err(e) => {
            error!("Failed to send debug log to Telegram: {}", e);
        }
    }
}