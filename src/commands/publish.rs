use crate::AppContext;
use tracing::{info, debug};

pub async fn run(ctx: &AppContext, category: String,) {
    info!("Starting PUBLISH process...");
    if ctx.is_debug {
        debug!("[DEBUG] Category: {:?}", category);
    }
    
    info!("PUBLISH process completed.");
}