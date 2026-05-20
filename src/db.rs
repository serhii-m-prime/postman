use crate::AppContext;
use rusqlite::{Connection, Result};
use tracing::{info};

pub struct RawArticle {
    pub id: i32,
    pub feed_name: String,
    pub title: String,
    pub description: Option<String>,
}

pub struct PublishableArticle {
    pub id: i32,
    pub feed_name: String, // Added field
    pub title: String,
    pub link: String,
    pub event_slug: String,
    pub score: i32,
}

pub fn get_connection(db_path: &str) -> Result<Connection> {
    let conn = Connection::open(db_path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(conn)
}

// Initialize DB and create tables if they don't exist
pub fn run_migrations(ctx: &AppContext) -> Result<&Connection> {
    let conn = &ctx.db;

    // We use UNIQUE on 'url' to automatically reject duplicate articles
    conn.execute(
        "CREATE TABLE IF NOT EXISTS articles (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            duplicate_of INTEGER DEFAULT NULL,
            is_leader BOOLEAN DEFAULT 0,
            feed_name TEXT NOT NULL,
            title TEXT NOT NULL,
            link TEXT NOT NULL UNIQUE, 
            description TEXT,
            category TEXT,
            pub_date DATETIME NOT NULL,
            event_slug TEXT DEFAULT NULL,
            score INTEGER DEFAULT NULL,
            summary TEXT DEFAULT NULL,
            is_sent BOOLEAN DEFAULT 0,
            is_published BOOLEAN DEFAULT 0,
            metadata TEXT DEFAULT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS feed_state (
            feed_name TEXT PRIMARY KEY,
            last_checked_at DATETIME NOT NULL,
            last_item_hash TEXT NOT NULL
        )",
        [],
    )?;

    // Add indexes for faster lookups
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_articles_event_slug ON articles(event_slug)",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_articles_status ON articles(is_sent, is_published, is_leader)",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_articles_duplicate_of ON articles(duplicate_of)",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_articles_feed_date ON articles(feed_name, pub_date)",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_articles_category ON articles(category, pub_date)",
        [],
    )?;

    info!("Database initialized.");
    Ok(&ctx.db)
}

pub fn insert_raw_article(
    ctx: &AppContext,
    feed_name: &str,
    title: &str,
    link: &str,
    description: Option<&str>,
    pub_date: &str,
) -> Result<usize> {
    let conn = &ctx.db;
    conn.execute(
        "INSERT OR IGNORE INTO articles (feed_name, title, link, description, pub_date) 
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![feed_name, title, link, description, pub_date],
    )
}

pub fn get_feed_last_hash(ctx: &AppContext, feed_name: &str) -> Result<Option<String>> {
    let conn = &ctx.db;
    let mut stmt = conn.prepare("SELECT last_item_hash FROM feed_state WHERE feed_name = ?1")?;
    
    let mut rows = stmt.query([feed_name])?;
    
    if let Some(row) = rows.next()? {
        let hash: String = row.get(0)?;
        Ok(Some(hash))
    } else {
        Ok(None)
    }
}

pub fn update_feed_state(ctx: &AppContext, feed_name: &str, last_hash: &str) -> Result<()> {
    let conn = &ctx.db;
    conn.execute(
        "INSERT INTO feed_state (feed_name, last_checked_at, last_item_hash) 
         VALUES (?1, CURRENT_TIMESTAMP, ?2)
         ON CONFLICT(feed_name) DO UPDATE SET 
            last_checked_at = CURRENT_TIMESTAMP,
            last_item_hash = EXCLUDED.last_item_hash",
        rusqlite::params![feed_name, last_hash],
    )?;
    Ok(())
}

pub fn get_unprocessed_articles(ctx: &AppContext) -> Result<Vec<RawArticle>> {
    let conn = &ctx.db;
    let mut stmt = conn.prepare(
        "SELECT id, feed_name, title, description 
         FROM articles 
         WHERE event_slug IS NULL 
         ORDER BY id ASC"
    )?;

    let article_iter = stmt.query_map([], |row| {
        Ok(RawArticle {
            id: row.get(0)?,
            feed_name: row.get(1)?,
            title: row.get(2)?,
            description: row.get(3)?,
        })
    })?;

    let mut articles = Vec::new();
    for article in article_iter {
        articles.push(article?);
    }

    Ok(articles)
}

pub fn update_processed_article(
    ctx: &AppContext,
    id: i32,
    score: i32,
    category: &str,
    event_slug: &str,
    reason: &str,
) -> rusqlite::Result<()> {
    let conn = &ctx.db;
    conn.execute(
        "UPDATE articles 
         SET score = ?, category = ?, summary = ?, event_slug = ?, is_sent = 0 
         WHERE id = ?",
        rusqlite::params![score, category, reason, event_slug,  id],
    )?;
    Ok(())
}

pub fn get_best_unpublished_articles(
    ctx: &AppContext, 
    target_category: &str
) -> Result<Vec<PublishableArticle>> {
    let conn = &ctx.db;
    
    // SQL Query logic:
    // 1. Filter out already processed (is_sent or is_published)
    // 2. Ensure AI has processed them (event_slug and score are NOT NULL)
    // 3. Filter by category OR ignore category if 'general' is requested
    // 4. Use ROW_NUMBER to partition by event_slug and sort by score DESC
    // 5. Select only rn = 1 (the highest scored article per event)
    let query = "
        SELECT id, feed_name, title, link, event_slug, score 
        FROM (
            SELECT id, feed_name, title, link, event_slug, score,
                   ROW_NUMBER() OVER(
                       PARTITION BY event_slug 
                       ORDER BY score DESC, pub_date DESC
                   ) as rn
            FROM articles
            WHERE is_published = 0 
              AND is_sent = 0 
              AND event_slug IS NOT NULL 
              AND score IS NOT NULL
              AND (category = ?1 OR ?1 = 'general')
        ) 
        WHERE rn = 1
        ORDER BY score DESC
        LIMIT ?2
    ";
    
    let mut stmt = conn.prepare(query)?;
    
    // Map the database row to our struct
    let article_iter = stmt.query_map([target_category, "1"], |row| {
        Ok(PublishableArticle {
            id: row.get(0)?,
            feed_name: row.get(1)?,
            title: row.get(2)?,
            link: row.get(3)?,
            event_slug: row.get(4)?,
            score: row.get(5)?,
        })
    })?;

    let mut articles = Vec::new();
    for article in article_iter {
        articles.push(article?);
    }

    Ok(articles)
}

pub async fn mark_as_published(ctx: &AppContext, article_id: i32) -> rusqlite::Result<()> {
    // TODO: Move this query to src/db.rs later
    let conn = &ctx.db;
    conn.execute(
        "UPDATE articles SET is_published = 1 WHERE id = ?1",
        rusqlite::params![article_id],
    )?;
    Ok(())
}

// Mark article as skipped to exclude it from future publish attempts
pub fn mark_article_as_skipped(
    ctx: &AppContext, 
    id: i32, 
    current_slug: &str
) -> Result<()> {
    let conn = &ctx.db;
    let skipped_slug = format!("{}-skipped", current_slug);
    
    // Set both is_published and is_sent to 1 to completely drop it from the pipeline
    conn.execute(
        "UPDATE articles 
         SET event_slug = ?1, is_published = 1, is_sent = 1 
         WHERE id = ?2",
        rusqlite::params![skipped_slug, id],
    )?;
    
    Ok(())
}