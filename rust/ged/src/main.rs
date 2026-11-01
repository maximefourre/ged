use std::process::ExitCode;

use clap::{Parser, Subcommand};

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
    match Cli::parse().command {
        Command::Serve => match serve().await {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("ged serve: {e}");
                ExitCode::FAILURE
            }
        },
        Command::Worker => not_implemented("worker"),
        Command::Migrate => not_implemented("migrate"),
        Command::Seed => not_implemented("seed"),
    }
}

fn not_implemented(name: &str) -> ExitCode {
    eprintln!("ged {name}: not implemented");
    ExitCode::FAILURE
}

async fn serve() -> std::io::Result<()> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    axum::serve(listener, ged::router()).await
}
