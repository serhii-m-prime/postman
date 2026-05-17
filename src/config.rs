use serde::Deserialize;
use std::fs;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub mysqlite_path: String,
    pub feeds: Vec<FeedConfig>,
    pub prompts: Prompts,
    pub filters: Filters,
    pub telegram: TelegramConfig,
}

#[derive(Debug, Deserialize)]
pub struct FeedConfig {
    pub name: String,
    pub url: String,
    pub poll_interval_min: u64,
}

#[derive(Debug, Deserialize)]
pub struct Prompts {
    pub scoring: String,
    pub enrichment: String,
}

#[derive(Debug, Deserialize)]
pub struct Filters {
    pub blacklist: Vec<BlacklistRule>,
}

#[derive(Debug, Deserialize)]
pub struct BlacklistRule {
    pub datafeed: String,
    pub categories: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct TelegramConfig {
    pub bot_token: String,
    pub main_channel_id: String,
    pub debug_channel_id: String,
}

impl Config {
    pub fn load(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let content = fs::read_to_string(path)?;
        let config: Config = serde_yaml::from_str(&content)?;
        Ok(config)
    }
}