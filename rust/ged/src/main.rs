use std::process::ExitCode;

use clap::{Parser, Subcommand};
use ged::config::Config;
use ged::{AppState, db, seed};

#[derive(Parser)]
#[command(name = "ged", about = "GED document management system")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the HTTP server
    Serve,
    /// Run the background job worker
    Worker,
    /// Run database migrations
    Migrate,
    /// Seed the database
    Seed,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let name = match cli.command {
        Command::Serve => "serve",
        Command::Worker => "worker",
        Command::Migrate => "migrate",
        Command::Seed => "seed",
    };
    let result = match cli.command {
        Command::Serve => serve().await,
        Command::Worker => Err("not implemented".into()),
        Command::Migrate => migrate().await,
        Command::Seed => seed_admin().await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ged {name}: {e}");
            ExitCode::FAILURE
        }
    }
}

type Failure = Box<dyn std::error::Error>;

/// Loads the configuration; a production misconfiguration stops here.
fn load_config() -> Result<Config, Failure> {
    let config = Config::from_process_env()?;
    for warning in &config.warnings {
        eprintln!("ged: warning: {warning}");
    }
    Ok(config)
}

fn database_url(config: &Config) -> Result<&str, Failure> {
    config
        .database_url
        .as_deref()
        .ok_or_else(|| "DATABASE_URL is not set".into())
}

async fn serve() -> Result<(), Failure> {
    let config = load_config()?;
    let pool = db::connect_lazy(database_url(&config)?)?;
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    axum::serve(listener, ged::router(AppState::new(config, pool))).await?;
    Ok(())
}

async fn migrate() -> Result<(), Failure> {
    let config = load_config()?;
    let pool = db::connect(database_url(&config)?).await?;
    db::migrate(&pool).await?;
    Ok(())
}

async fn seed_admin() -> Result<(), Failure> {
    let config = load_config()?;
    let pool = db::connect(database_url(&config)?).await?;
    seed::run(&pool, &config).await?;
    Ok(())
}
