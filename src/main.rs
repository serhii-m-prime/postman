mod config;
mod commands;
mod db;

use clap::{Parser, Subcommand};
use rusqlite::Connection;
use crate::config::Config;
use tracing::{error, info, Level};
use tracing_subscriber::fmt::writer::MakeWriterExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Parser)]
#[command(name = "postman", version = "1.0", about = "AI News pipeline with Optimization by LLMs")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

pub struct AppContext {
    pub config: Config,
    pub db: Connection,
    pub is_debug: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Handle RSS/Atom feeds and stroe them in the database
    Fetch {
        #[arg(long = "debug")]
        is_debug: bool,
        #[arg(long)]
        data_feed: Option<String>,
    },
    /// Process news queue through Gemini API
    Process {
        #[arg(long = "debug")]
        is_debug: bool,
        #[arg(long)]
        article_id: Option<u64>,
    },
    /// Publish digest in Telegram
    Publish {
        #[arg(long = "debug")]
        is_debug: bool,
        #[arg(long)]
        category: String,
    },
    /// Generate analytics for sources
    Stats {
        #[arg(long = "debug")]
        is_debug: bool,
        #[arg(long, default_value = "7")]
        period: String,
    },
    /// Install systemd timers
    Install,
}

fn init_logger() {
    // Logger: create files app.log.YYYY-MM-DD in dir logs/, save 3 days (TODO: move variable to config)
    let file_appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .max_log_files(3)
        .filename_prefix("app.log")
        .build("logs")
        .expect("Failed to create file appender");

    // Logger: format for console (without colors, to avoid clutter in systemd)
    let stdout = std::io::stdout.with_max_level(Level::DEBUG);

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("info,postman=debug"))
        .with(tracing_subscriber::fmt::layer().with_writer(stdout))
        .with(tracing_subscriber::fmt::layer().with_writer(file_appender).with_ansi(false))
        .init();
}

#[tokio::main]
async fn main() {
    init_logger();
    info!("Postman initializing...");

    let cli = Cli::parse();

    let is_debug = match &cli.command {
        Commands::Fetch { is_debug, .. } => *is_debug,
        Commands::Process { is_debug, .. } => *is_debug,
        Commands::Publish { is_debug, .. } => *is_debug,
        Commands::Stats { is_debug, .. } => *is_debug,
        Commands::Install => false,
    };

    let config = match config::Config::load("config.yaml") {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to load config.yaml: {}", e);
            std::process::exit(1);
        }
    };

// Initialize SQLite Database
    let db_conn = match db::get_connection(&config.mysqlite_path) {
        Ok(conn) => conn,
        Err(e) => {
            error!("Failed to initialize database: {}", e);
            std::process::exit(1);
        }
    };

    let ctx = AppContext {
                config,
                db: db_conn,
                is_debug,
    };
    

    match cli.command {
        Commands::Fetch { data_feed, .. } => {
            info!("Fetch task triggered. Debug: {}, Data feed: {:?}", ctx.is_debug, data_feed);
            commands::fetch::run(&ctx, data_feed).await;
        }
        Commands::Process { article_id, .. } => {
            info!("Process task triggered. Debug: {}, Article ID: {:?}", ctx.is_debug, article_id);
            commands::process::run(&ctx, article_id).await;
        }
        Commands::Publish { category, .. } => {
            info!("Publish task triggered. Debug: {}, Category: {}", ctx.is_debug, category);
            commands::publish::run(&ctx, category).await;
        }
        Commands::Stats { period, .. } => {
            info!("Stats task triggered. Debug: {}, Period: {}", ctx.is_debug, period);
            commands::stats::run(&ctx, period).await;
        }
        Commands::Install => {
            info!("Install task triggered.");
            commands::install::run(&ctx).await;
        }
    }
}