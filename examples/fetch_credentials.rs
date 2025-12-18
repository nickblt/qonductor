//! Fetch and display Qobuz app credentials from web.
//!
//! Run with: cargo run --example fetch_credentials
//! Run with debug: RUST_LOG=qonductor=debug cargo run --example fetch_credentials
//! Run with trace: RUST_LOG=qonductor=trace cargo run --example fetch_credentials

use qonductor::credentials::fetch_app_credentials;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    // Initialize tracing from RUST_LOG env var
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    println!("Fetching Qobuz app credentials from web...\n");

    match fetch_app_credentials().await {
        Ok(creds) => {
            println!("\nSuccess!");
            println!("  app_id:     {}", creds.app_id);
            println!("  app_secret: {}", creds.app_secret);
        }
        Err(e) => {
            eprintln!("\nError: {e}");
            std::process::exit(1);
        }
    }
}
