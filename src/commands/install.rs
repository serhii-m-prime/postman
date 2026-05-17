use crate::AppContext;
use crate::db;
use tracing::{info, error};

pub async fn run(ctx: &AppContext) {
    info!("Starting Postman infrastructure installation...");

    if let Err(e) = db::run_migrations(&ctx) {
        error!("Database schema installation failed: {}", e);
        std::process::exit(1);
    }

    // TODO: create and install systemd timer files
    // info!("Installing systemd timers...");

    info!("Installation phase completed. System is ready for runtime processes.");
}