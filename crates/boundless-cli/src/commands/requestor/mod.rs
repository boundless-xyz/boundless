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

//! Commands for requestors interacting with the Boundless market.

mod balance;
mod deposit;
mod get_proof;
mod status;
mod submit;
mod submit_offer;
mod verify_proof;
mod withdraw;

pub use balance::RequestorBalance;
pub use deposit::RequestorDeposit;
pub use get_proof::RequestorGetProof;
pub use status::RequestorStatus;
pub use submit::RequestorSubmit;
pub use submit_offer::RequestorSubmitOffer;
pub use verify_proof::RequestorVerifyProof;
pub use withdraw::RequestorWithdraw;

use clap::Subcommand;

use crate::config::GlobalConfig;

/// Commands for requestors
#[derive(Subcommand, Clone, Debug)]
pub enum RequestorCommands {
    /// Deposit funds into the market
    Deposit(RequestorDeposit),
    /// Withdraw funds from the market
    Withdraw(RequestorWithdraw),
    /// Check the balance of an account in the market
    #[command(name = "deposited-balance")]
    DepositedBalance(RequestorBalance),
    /// Check the balance of an account (alias for deposited-balance)
    Balance(RequestorBalance),
    /// Submit a fully specified proof request
    Submit(RequestorSubmit),
    /// Submit a proof request constructed with the given offer, input, and image
    #[command(name = "submit-offer")]
    SubmitOffer(Box<RequestorSubmitOffer>),
    /// Get the status of a given request
    Status(RequestorStatus),
    /// Get the journal and seal for a given request
    #[command(name = "get-proof")]
    GetProof(RequestorGetProof),
    /// Verify the proof of the given request
    #[command(name = "verify-proof")]
    VerifyProof(RequestorVerifyProof),
}

impl RequestorCommands {
    /// Run the command
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        match self {
            Self::Deposit(cmd) => cmd.run(global_config).await,
            Self::Withdraw(cmd) => cmd.run(global_config).await,
            Self::DepositedBalance(cmd) | Self::Balance(cmd) => cmd.run(global_config).await,
            Self::Submit(cmd) => cmd.run(global_config).await,
            Self::SubmitOffer(cmd) => cmd.run(global_config).await,
            Self::Status(cmd) => cmd.run(global_config).await,
            Self::GetProof(cmd) => cmd.run(global_config).await,
            Self::VerifyProof(cmd) => cmd.run(global_config).await,
        }
    }
}
