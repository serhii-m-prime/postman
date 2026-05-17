use crate::AppContext;
use tracing::{info, debug};

pub async fn run(ctx: &AppContext, feed_id: Option<String>) {
    info!("Starting FETCH process...");
    if ctx.is_debug {
        debug!("[DEBUG] Feed filter: {:?} CONFIG: {:?}", feed_id, ctx.config);
    }

    info!("FETCH process completed.");
}

fn calculate_hash(text: &str) -> String {
    let digest = md5::compute(text.as_bytes());
    format!("{:x}", digest)
}