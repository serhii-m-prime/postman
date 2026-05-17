use crate::AppContext;
use tracing::{info, debug};

pub async fn run(ctx: &AppContext, article_id: Option<u64>) {
    info!("Starting processing...");
    if ctx.is_debug {
        debug!("[DEBUG] article id: {:?}", article_id);
    }
    
    info!("Processing completed.");
}