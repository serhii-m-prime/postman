use crate::config::Config;
use tracing::{info, debug};

pub async fn run(config: &Config, debug_mode: bool, feed_id: Option<String>) {
    info!("Starting FETCH process...");
    if debug_mode {
        debug!("[DEBUG] Feed filter: {:?} CONFIG: {:?}", feed_id, config);
    }

    info!("FETCH process completed.");
}