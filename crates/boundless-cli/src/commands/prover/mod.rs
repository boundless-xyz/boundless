// Copyright 2025 RISC Zero, Inc.
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
mod deposit_collateral;
mod execute;
mod fulfill;
mod lock;
mod withdraw_collateral;

pub use balance_collateral::ProverBalanceCollateral;
pub use benchmark::ProverBenchmark;
pub use deposit_collateral::ProverDepositCollateral;
pub use execute::ProverExecute;
pub use fulfill::ProverFulfill;
pub use lock::ProverLock;
pub use withdraw_collateral::ProverWithdrawCollateral;

use clap::Subcommand;

use crate::config::GlobalConfig;

/// Commands for provers
#[derive(Subcommand, Clone, Debug)]
pub enum ProverCommands {
    /// Deposit collateral funds into the market
    #[command(name = "deposit-collateral")]
    DepositCollateral(ProverDepositCollateral),
    /// Withdraw collateral funds from the market
    #[command(name = "withdraw-collateral")]
    WithdrawCollateral(ProverWithdrawCollateral),
    /// Check the collateral balance of an account in the market
    #[command(name = "deposited-balance-collateral")]
    DepositedBalanceCollateral(ProverBalanceCollateral),
    /// Check the collateral balance of an account (alias)
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
}

impl ProverCommands {
    /// Run the command
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        match self {
            Self::DepositCollateral(cmd) => cmd.run(global_config).await,
            Self::WithdrawCollateral(cmd) => cmd.run(global_config).await,
            Self::DepositedBalanceCollateral(cmd) | Self::BalanceCollateral(cmd) => {
                cmd.run(global_config).await
            }
            Self::Lock(cmd) => cmd.run(global_config).await,
            Self::Fulfill(cmd) => cmd.run(global_config).await,
            Self::Execute(cmd) => cmd.run(global_config).await,
            Self::Benchmark(cmd) => cmd.run(global_config).await,
        }
    }
}
