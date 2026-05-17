use crate::AppContext;
use rusqlite::{Connection, Result};
use tracing::{info};

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