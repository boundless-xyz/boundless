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

//! Common test utilities for rewards integration tests.

use std::path::Path;

use boundless_test_utils::povw::make_work_claim;
use risc0_povw::PovwLogId;
use risc0_zkvm::{FakeReceipt, GenericReceipt, ReceiptClaim, WorkClaim};

/// Make a fake work receipt with the given log ID and a random job number, encode it, and save it to a file.
pub fn make_fake_work_receipt_file(
    log_id: PovwLogId,
    value: u64,
    segments: u32,
    path: impl AsRef<Path>,
) -> anyhow::Result<()> {
    let work_claim = make_work_claim((log_id, rand::random()), segments, value)?;
    let work_receipt: GenericReceipt<WorkClaim<ReceiptClaim>> = FakeReceipt::new(work_claim).into();
    std::fs::write(path.as_ref(), bincode::serialize(&work_receipt)?)?;
    Ok(())
}

/// Environment variables for rewards commands with new prefixes
pub struct RewardsEnv {
    pub reward_rpc_url: String,
    pub reward_private_key: String,
    pub povw_accounting_address: String,
    pub povw_mint_address: String,
    pub zkc_address: String,
    pub vezkc_address: String,
}

impl RewardsEnv {
    /// Create environment variables from test context
    pub async fn from_test_ctx(
        ctx: &boundless_test_utils::povw::TestCtx,
        tx_signer: &alloy::signers::local::PrivateKeySigner,
    ) -> Self {
        Self {
            reward_rpc_url: ctx.anvil.lock().await.endpoint_url().to_string(),
            reward_private_key: format!("{:#x}", tx_signer.to_bytes()),
            povw_accounting_address: format!("{:#x}", ctx.povw_accounting.address()),
            povw_mint_address: format!("{:#x}", ctx.povw_mint.address()),
            zkc_address: format!("{:#x}", ctx.zkc.address()),
            vezkc_address: format!("{:#x}", ctx.zkc_rewards.address()),
        }
    }

    /// Apply these environment variables to a Command
    pub fn apply_to_cmd<'a>(
        &self,
        cmd: &'a mut assert_cmd::Command,
    ) -> &'a mut assert_cmd::Command {
        cmd.env("REWARD_RPC_URL", &self.reward_rpc_url)
            .env("REWARD_PRIVATE_KEY", &self.reward_private_key)
            .env("POVW_ACCOUNTING_ADDRESS", &self.povw_accounting_address)
            .env("POVW_MINT_ADDRESS", &self.povw_mint_address)
            .env("ZKC_ADDRESS", &self.zkc_address)
            .env("VEZKC_ADDRESS", &self.vezkc_address)
            .env("RISC0_DEV_MODE", "1")
            .env("NO_COLOR", "1")
            .env("RUST_LOG", "boundless_cli=debug,info")
    }
}

/// Helper to create a Command for the boundless CLI
pub fn cli_cmd() -> anyhow::Result<assert_cmd::Command> {
    Ok(assert_cmd::Command::cargo_bin("boundless")?)
}
