use crate::AppContext;
use crate::db;
use crate::db::PublishableArticle;
use tracing::{info, debug, error, warn};
use serde_json::json;

pub async fn run(ctx: &AppContext, category: String) {
    info!("Starting PUBLISH process for category: {}", category);

    // Stage 1: Select the best unprocessed articles from DB
    let articles = select_articles(ctx, &category).await;
    
    if articles.is_empty() {
        info!("No new articles found for publishing in category: {}", category);
        return;
    }

    // Initialize HTTP client once for all API calls (Scraping, Gemini, Telegram)
    let client = reqwest::Client::new();

    for article in articles {
        debug!("Processing article ID: {} ({})", article.id, article.event_slug);

        // Stage 2: Scrape full article content
        let content = match scrape_content(&client, &article.link).await {
            Ok(text) => text,
            Err(e) => {
                error!("Failed to scrape article {}: {}", article.id, e);
                continue; // Skip to next article if scraping fails
            }
        };

        // Stage 3: Summarize content via LLM (Gemini API)
        let summary = match summarize_article(ctx, &client, &content).await {
            Ok(sum) => sum,
            Err(e) => {
                error!("Failed to summarize article {}: {}", article.id, e);
                continue;
            }
        };

        // Stage 4: Send to Telegram (Main or Debug channel based on ctx.is_debug)
        let is_sent = send_to_telegram(ctx, &client, &article, &summary).await;

        // Stage 5: Update database status
        if is_sent {
            if let Err(e) = mark_as_published(ctx, article.id).await {
                error!("Failed to update DB status for article {}: {}", article.id, e);
            } else {
                info!("Successfully published article ID: {}", article.id);
            }
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
async fn scrape_content(_client: &reqwest::Client, _url: &str) -> Result<String, Box<dyn std::error::Error>> {
    // TODO: Implement scraping using `wreq` (from your Cargo.toml) or `reqwest`.
    // 1. Fetch HTML page
    // 2. Extract meaningful text (e.g., using a readability crate or basic HTML tag stripping)
    // 3. Truncate text if it's too large for LLM context limits
    
    Ok("Mocked scraped content...".to_string())
}

// --- STAGE 3: LLM Summarization ---
async fn summarize_article(
    ctx: &AppContext, 
    _client: &reqwest::Client, 
    _text: &str
) -> Result<String, Box<dyn std::error::Error>> {
    // TODO: Implement Gemini API call
    // 1. Use ctx.config.prompts.enrichment as System Prompt
    // 2. Send scraped text as User Prompt
    // 3. Parse JSON response to extract the summary
    
    Ok("Mocked summarized text...".to_string())
}

// --- STAGE 4: Telegram Integration ---
async fn send_to_telegram(
    ctx: &AppContext, 
    client: &reqwest::Client, 
    article: &PublishableArticle, 
    summary: &str
) -> bool {
    let token = &ctx.config.telegram.bot_token;
    let channel_id = if ctx.is_debug {
        &ctx.config.telegram.debug_channel_id
    } else {
        &ctx.config.telegram.main_channel_id
    };

    // Format the final message (e.g., MarkdownV2 or HTML)
    let message = format!(
        "<b>{}</b>\n\n{}\n\n<a href=\"{}\">Читати оригінал</a>", 
        article.title, summary, article.link
    );

    let url = format!("https://api.telegram.org/bot{}/sendMessage", token);
    
    let payload = json!({
        "chat_id": channel_id,
        "text": message,
        "parse_mode": "HTML",
        "disable_web_page_preview": false
    });

    // TODO: Execute POST request using client
    // let resp = client.post(&url).json(&payload).send().await;
    // Check if resp.status().is_success() and return true/false

    if ctx.is_debug {
        debug!("[DEBUG] Simulated sending to TG Channel {}:\n{}", channel_id, message);
    }

    true // Return true on success
}

// --- STAGE 5: Database Update ---
async fn mark_as_published(ctx: &AppContext, article_id: i32) -> rusqlite::Result<()> {
    // TODO: Move this query to src/db.rs later
    let conn = &ctx.db;
    conn.execute(
        "UPDATE articles SET is_published = 1 WHERE id = ?1",
        rusqlite::params![article_id],
    )?;
    Ok(())
}