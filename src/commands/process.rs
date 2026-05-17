use crate::config::Config;
use tracing::{info, debug};

pub async fn run(config: &Config, debug_mode: bool, article_id: Option<u64>) {
    info!("Starting processing...");
    if debug_mode {
        debug!("[DEBUG] article id: {:?}", article_id);
    }
    
    info!("Processing completed.");
}