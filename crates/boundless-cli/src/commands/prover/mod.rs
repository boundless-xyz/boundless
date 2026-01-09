// Copyright 2026 Boundless Foundation, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Commands for provers interacting with the Boundless market.

mod balance_collateral;
mod benchmark;
mod config;
mod deposit_collateral;
mod execute;
mod fulfill;
mod generate_config;
mod lock;
mod slash;
mod withdraw_collateral;

pub use balance_collateral::ProverBalanceCollateral;
pub use benchmark::ProverBenchmark;
pub use config::ProverConfigCmd;
pub use deposit_collateral::ProverDepositCollateral;
pub use execute::ProverExecute;
pub use fulfill::ProverFulfill;
pub use generate_config::ProverGenerateConfig;
pub use lock::ProverLock;
pub use slash::ProverSlash;
pub use withdraw_collateral::ProverWithdrawCollateral;

use clap::Subcommand;

use crate::{commands::setup::ProverSetup, config::GlobalConfig};

/// Commands for provers
#[derive(Subcommand, Clone, Debug)]
#[command(after_help = "\x1b[1;4mModule Configuration:\x1b[0m
  Run 'boundless prover setup' for interactive setup

  Alternatively set environment variables:
    \x1b[1mPROVER_RPC_URL\x1b[0m            RPC endpoint for prover module
    \x1b[1mPROVER_PRIVATE_KEY\x1b[0m        Private key for prover transactions
    \x1b[1mBOUNDLESS_MARKET_ADDRESS\x1b[0m  Market contract address (optional, has default)
    \x1b[1mSET_VERIFIER_ADDRESS\x1b[0m      Verifier contract address (optional, has default)

  Or configure while executing commands:
    Example: \x1b[1mboundless prover balance --prover-rpc-url <url> --prover-private-key <key>\x1b[0m")]
pub enum ProverCommands {
    /// Show prover configuration status
    Config(ProverConfigCmd),
    /// Deposit collateral funds into the market
    #[command(name = "deposit-collateral")]
    DepositCollateral(ProverDepositCollateral),
    /// Withdraw collateral funds from the market
    #[command(name = "withdraw-collateral")]
    WithdrawCollateral(ProverWithdrawCollateral),
    /// Check the collateral balance of an account
    #[command(name = "balance-collateral")]
    BalanceCollateral(ProverBalanceCollateral),
    /// Lock a request in the market
    Lock(ProverLock),
    /// Fulfill one or more proof requests
    Fulfill(ProverFulfill),
    /// Execute a proof request using the RISC Zero zkVM executor
    Execute(ProverExecute),
    /// Benchmark proof requests
    Benchmark(ProverBenchmark),
    /// Slash a prover for a given request
    Slash(ProverSlash),
    /// Interactive setup wizard for prover configuration
    Setup(ProverSetup),
    /// Generate optimized broker and compose configuration files
    #[command(name = "generate-config")]
    GenerateConfig(ProverGenerateConfig),
}

impl ProverCommands {
    /// Run the command
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        match self {
            Self::Config(cmd) => cmd.run(global_config).await,
            Self::DepositCollateral(cmd) => cmd.run(global_config).await,
            Self::WithdrawCollateral(cmd) => cmd.run(global_config).await,
            Self::BalanceCollateral(cmd) => cmd.run(global_config).await,
            Self::Lock(cmd) => cmd.run(global_config).await,
            Self::Fulfill(cmd) => cmd.run(global_config).await,
            Self::Execute(cmd) => cmd.run(global_config).await,
            Self::Benchmark(cmd) => {
                cmd.run(global_config).await?;
                Ok(())
            }
            Self::Slash(cmd) => cmd.run(global_config).await,
            Self::Setup(cmd) => cmd.run(global_config).await,
            Self::GenerateConfig(cmd) => cmd.run(global_config).await,
        }
    }
}
