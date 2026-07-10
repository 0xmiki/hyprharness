use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .init();

    if let Err(error) = hyprharness::cli::run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

use std::io::IsTerminal;
