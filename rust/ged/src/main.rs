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
    /// Perform one job in the foreground (`Job.perform_now`)
    RunJob {
        /// Job class name, e.g. `PurgeDeletedDocumentsJob`
        kind: String,
        /// JSON arguments, e.g. `{"upload_request_id": "..."}`
        args: Option<String>,
    },
    /// Run database migrations
    Migrate,
    /// Seed the database
    Seed,
}

#[tokio::main]
async fn main() -> ExitCode {
    // Outlives `serve`. SIGTERM or SIGINT makes `serve` return, then this
    // drop flushes the exporters (Rails does the same from `at_exit`).
    let _telemetry = ged::telemetry::install_from_env();
    let cli = Cli::parse();
    let name = match cli.command {
        Command::Serve => "serve",
        Command::Worker => "worker",
        Command::RunJob { .. } => "run-job",
        Command::Migrate => "migrate",
        Command::Seed => "seed",
    };
    let result = match cli.command {
        Command::Serve => serve().await,
        Command::Worker => worker().await,
        Command::RunJob { kind, args } => run_job(&kind, args.as_deref()).await,
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
    axum::serve(
        listener,
        ged::telemetry::wrap(ged::router(AppState::new(config, pool))),
    )
    .with_graceful_shutdown(ged::telemetry::wait_for_shutdown_signal())
    .await?;
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

async fn worker() -> Result<(), Failure> {
    let config = load_config()?;
    ged::jobs::worker::run(
        database_url(&config)?,
        ged::jobs::worker::WorkerSettings::from_process_env(),
        config.max_upload_bytes,
    )
    .await
}

async fn run_job(kind: &str, args: Option<&str>) -> Result<(), Failure> {
    let config = load_config()?;
    ged::jobs::worker::perform_now(database_url(&config)?, kind, args, config.max_upload_bytes)
        .await
}
