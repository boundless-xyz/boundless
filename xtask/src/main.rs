use clap::{Parser, Subcommand};

mod setup_blake3_groth16;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand)]
enum Commands {
    SetupBlake3Groth16(crate::setup_blake3_groth16::SetupBlake3Groth16),
}

impl Commands {
    fn run(&self) {
        match self {
            Commands::SetupBlake3Groth16(cmd) => cmd.run(),
        }
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::filter::EnvFilter::from_default_env())
        .init();

    Cli::parse().cmd.run();
}
