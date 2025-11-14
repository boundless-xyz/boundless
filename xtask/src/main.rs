use clap::{Parser, Subcommand};

mod bootstrap_blake3_groth16;
mod setup_blake3_groth16;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand)]
enum Commands {
    SetupBlake3Groth16(crate::setup_blake3_groth16::SetupBlake3Groth16),
    BootstrapBlake3Groth16(crate::bootstrap_blake3_groth16::BootstrapBlake3Groth16),
}

impl Commands {
    async fn run(&self) {
        match self {
            Commands::SetupBlake3Groth16(cmd) => cmd.run(),
            Commands::BootstrapBlake3Groth16(cmd) => cmd.run().await,
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::filter::EnvFilter::from_default_env())
        .init();

    Cli::parse().cmd.run().await;
}
