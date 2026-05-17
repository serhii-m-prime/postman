use crate::config::Config;
use tracing::{info, debug};

pub async fn run(config: &Config, debug_mode: bool, period: String,) {
    info!("Starting STATS process...");
    if debug_mode {
        debug!("[DEBUG] Period: {:?}", period);
    }
    // TODO: Обхід RSS
    info!("STATS process completed.");
}