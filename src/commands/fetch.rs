// src/commands/fetch.rs

use crate::AppContext;
use crate::db;
use tracing::{info, error, warn, debug};
use wreq::Client;
use wreq_util::Emulation;

pub async fn run(ctx: &AppContext, target_feed: Option<String>) {
    info!("Starting FETCH process with modern browser emulation (wreq)...");

    let feeds_to_process: Vec<_> = match &target_feed {
        Some(name) => ctx.config.feeds.iter().filter(|f| &f.name == name).collect(),
        None => ctx.config.feeds.iter().collect(),
    };

    if feeds_to_process.is_empty() {
        warn!("No matching feeds found in config for target: {:?}", target_feed);
        return;
    }

    let client = match Client::builder()
        .emulation(Emulation::Chrome137)
        .redirect(wreq::redirect::Policy::limited(5))
        .timeout(std::time::Duration::from_secs(15))
        .build() 
    {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to build wreq emulation client: {}", e);
            return;
        }
    };

    // 3. Обхід джерела
    for feed in feeds_to_process {
        info!("Fetching feed: {} ({})", feed.name, feed.url);
        
        match client.get(&feed.url).send().await {
            Ok(response) => {
                if !response.status().is_success() {
                    error!("HTTP error {} for feed {}", response.status(), feed.name);
                    continue;
                }

                let bytes = match response.bytes().await {
                    Ok(b) => b,
                    Err(e) => {
                        error!("Failed to read bytes for feed {}: {}", feed.name, e);
                        continue;
                    }
                };

                // Парсинг XML вмісту
                let parsed_feed = match feed_rs::parser::parse(&bytes[..]) {
                    Ok(f) => f,
                    Err(e) => {
                        error!("Failed to parse XML for feed {}: {}", feed.name, e);
                        continue;
                    }
                };

                if let Some(latest_entry) = parsed_feed.entries.first() {
                    let latest_link = latest_entry.links.first().map(|l| l.href.as_str()).unwrap_or("");
                    let current_hash = calculate_hash(latest_link);

                    match db::get_feed_last_hash(ctx, &feed.name) {
                        Ok(Some(saved_hash)) if saved_hash == current_hash => {
                            info!("Feed '{}' has not changed since last check. Skipping details.", feed.name);
                            continue; 
                        }
                        Ok(_) => {
                            debug!("Feed '{}' has new content or checked first time.", feed.name);
                        }
                        Err(e) => {
                            warn!("Failed to read feed state from DB for '{}': {}", feed.name, e);
                        }
                    }

                    let mut inserted_count = 0;
                    for entry in &parsed_feed.entries {
                        let title = entry.title.as_ref().map(|t| t.content.as_str()).unwrap_or("No Title");
                        let link = entry.links.first().map(|l| l.href.as_str()).unwrap_or("");
                        
                        let description_cleaned = entry.summary.as_ref()
                            .map(|s| sanitize_html(&s.content));
                        
                        let pub_date = entry.updated.or(entry.published)
                            .map(|dt| dt.to_rfc3339())
                            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());

                        match db::insert_raw_article(ctx, &feed.name, title, link, description_cleaned.as_deref(), &pub_date) {
                            Ok(0) => debug!("Article already exists: {}", link), 
                            Ok(_) => inserted_count += 1, 
                            Err(e) => error!("Failed to save article to DB: {}", e),
                        }
                    }

                    info!("Processed '{}'. Inserted {} new articles.", feed.name, inserted_count);

                    if let Err(e) = db::update_feed_state(ctx, &feed.name, &current_hash) {
                        error!("Failed to update feed state in DB for '{}': {}", feed.name, e);
                    }
                } else {
                    warn!("Feed '{}' is empty.", feed.name);
                }
            }
            Err(e) => {
                error!("Emulated HTTP request failed for feed {}: {}", feed.name, e);
            }
        }
    }

    info!("FETCH process completed successfully.");
}

fn sanitize_html(html: &str) -> String {
    let mut cleaned = String::with_capacity(html.len());
    let mut in_tag = false;

    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ => {
                if !in_tag {
                    cleaned.push(c);
                }
            }
        }
    }

    let decoded = html_escape::decode_html_entities(&cleaned).into_owned();

    decoded
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ")
}

fn calculate_hash(text: &str) -> String {
    let digest = md5::compute(text.as_bytes());
    format!("{:x}", digest)
}