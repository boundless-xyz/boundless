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

use alloy::providers::{Provider, ProviderBuilder};
use alloy_node_bindings::AnvilInstance;
use assert_cmd::Command;
use boundless_market::deployments::Deployment as MarketDeployment;
use boundless_zkc::deployments::Deployment as ZkcDeployment;
use std::{borrow::Cow, collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

// Anvil's pre-funded accounts with private keys
pub const ANVIL_ACCOUNTS: &[(&str, &str)] = &[
    (
        "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266",
        "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
    ),
    (
        "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
        "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
    ),
    (
        "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC",
        "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a",
    ),
    (
        "0x90F79bf6EB2c4f870365E785982E1f101E93b906",
        "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6",
    ),
    (
        "0x15d34AAf54267DB7D7c367839AAf71A00a2C6A65",
        "0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a",
    ),
    (
        "0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc",
        "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba",
    ),
    (
        "0x976EA74026E726554dB657fA54763abd0C3a0aa9",
        "0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e",
    ),
    (
        "0x14dC79964da2C08b23698B3D3cc7Ca32193d9955",
        "0x4bbbf85ce3377467afe5d46f804f221813b2bb87f24d81f60f1fcdbf7cbf4356",
    ),
    (
        "0x23618e81E3f5cdF7f54C3d65f7FBc0aBf5B21E8f",
        "0xdbda1821b80551c9d65939329250298aa3472ba22feea921c0cf5d620ea67b97",
    ),
    (
        "0xa0Ee7A142d267C1f36714E4a8F75612F20a79720",
        "0x2a871d0798f97d79848a013d4936a73bf4cc922c825d33c1cf7073dff6d409c6",
    ),
];

// Test context with Anvil instance and deployed contracts
pub struct TestContext {
    pub anvil: AnvilInstance,
    pub endpoint: String,
    pub chain: Chain,
    pub market_deployment: Option<MarketDeployment>,
    pub zkc_deployment: Option<ZkcDeployment>,
}

#[derive(Clone, Copy)]
pub enum Chain {
    Base,
    Ethereum,
}

impl TestContext {
    pub async fn base() -> Self {
        let anvil = alloy_node_bindings::Anvil::new().spawn();
        let endpoint = anvil.endpoint().to_string();

        let test_ctx = boundless_test_utils::market::create_test_ctx(&anvil)
            .await
            .expect("Failed to deploy market contracts");

        Self {
            anvil,
            endpoint,
            chain: Chain::Base,
            market_deployment: Some(test_ctx.deployment),
            zkc_deployment: None,
        }
    }

    pub async fn ethereum() -> Self {
        let anvil = alloy_node_bindings::Anvil::new().spawn();
        let endpoint = anvil.endpoint().to_string();

        let test_ctx = boundless_test_utils::zkc::test_ctx_with(Arc::new(Mutex::new(anvil)), 0)
            .await
            .expect("Failed to deploy ZKC contracts");

        let anvil_instance =
            Arc::try_unwrap(test_ctx.anvil).expect("Failed to unwrap Arc").into_inner();

        Self {
            anvil: anvil_instance,
            endpoint,
            chain: Chain::Ethereum,
            market_deployment: None,
            zkc_deployment: Some(test_ctx.deployment),
        }
    }

    // Get an Anvil account with private key
    pub fn account(&self, index: usize) -> TestAccount {
        let (addr, key) = ANVIL_ACCOUNTS[index];
        TestAccount { address: addr.to_string(), private_key: key.to_string(), index }
    }

    // Impersonate any address for transfers
    pub async fn impersonate(&self, address: &str) -> anyhow::Result<()> {
        let provider = ProviderBuilder::new().connect_http(self.endpoint.parse()?);

        provider
            .raw_request::<_, ()>(Cow::Borrowed("anvil_impersonateAccount"), vec![address])
            .await?;
        Ok(())
    }

    pub async fn stop_impersonating(&self, address: &str) -> anyhow::Result<()> {
        let provider = ProviderBuilder::new().connect_http(self.endpoint.parse()?);

        provider
            .raw_request::<_, ()>(Cow::Borrowed("anvil_stopImpersonatingAccount"), vec![address])
            .await?;
        Ok(())
    }

    pub fn cmd(&self, group: &str, command: &str) -> BoundlessCmd {
        BoundlessCmd::new(group, command).with_context(self)
    }
}

pub struct TestAccount {
    pub address: String,
    pub private_key: String,
    pub index: usize,
}

// Builder pattern for CLI commands
pub struct BoundlessCmd {
    args: Vec<String>,
    env_vars: HashMap<String, String>,
}

impl BoundlessCmd {
    pub fn new(group: &str, command: &str) -> Self {
        Self { args: vec![group.to_string(), command.to_string()], env_vars: HashMap::new() }
    }

    pub fn arg(mut self, arg: &str) -> Self {
        self.args.push(arg.to_string());
        self
    }

    pub fn args(mut self, args: &[&str]) -> Self {
        self.args.extend(args.iter().map(|s| s.to_string()));
        self
    }

    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.env_vars.insert(key.to_string(), value.to_string());
        self
    }

    pub fn envs(mut self, envs: &[(&str, &str)]) -> Self {
        for (key, value) in envs {
            self.env_vars.insert(key.to_string(), value.to_string());
        }
        self
    }

    pub fn with_context(self, ctx: &TestContext) -> Self {
        match ctx.chain {
            Chain::Base => self.with_base_config(ctx),
            Chain::Ethereum => self.with_ethereum_config(ctx),
        }
    }

    pub fn with_base_config(self, ctx: &TestContext) -> Self {
        let deployment = ctx.market_deployment.as_ref().expect("Market deployment not initialized");
        self.env("REQUESTOR_RPC_URL", &ctx.endpoint)
            .env("PROVER_RPC_URL", &ctx.endpoint)
            .env("BOUNDLESS_MARKET_ADDRESS", &deployment.boundless_market_address.to_string())
            .env("SET_VERIFIER_ADDRESS", &deployment.set_verifier_address.to_string())
    }

    pub fn with_ethereum_config(self, ctx: &TestContext) -> Self {
        let deployment = ctx.zkc_deployment.as_ref().expect("ZKC deployment not initialized");
        self.env("REWARD_RPC_URL", &ctx.endpoint)
            .env("ETH_MAINNET_RPC_URL", &ctx.endpoint)
            .env("ZKC_ADDRESS", &deployment.zkc_address.to_string())
            .env("VEZKC_ADDRESS", &deployment.vezkc_address.to_string())
            .env("STAKING_REWARDS_ADDRESS", &deployment.staking_rewards_address.to_string())
    }

    pub fn with_account(self, account: &TestAccount) -> Self {
        self.env("REQUESTOR_PRIVATE_KEY", &account.private_key)
            .env("PROVER_PRIVATE_KEY", &account.private_key)
            .env("REWARD_PRIVATE_KEY", &account.private_key)
            .env("STAKING_PRIVATE_KEY", &account.private_key)
    }

    pub fn with_private_key(self, key: &str) -> Self {
        self.env("REQUESTOR_PRIVATE_KEY", key)
            .env("PROVER_PRIVATE_KEY", key)
            .env("REWARD_PRIVATE_KEY", key)
            .env("STAKING_PRIVATE_KEY", key)
    }

    pub fn assert(self) -> assert_cmd::assert::Assert {
        let mut cmd = Command::cargo_bin("boundless").unwrap();
        cmd.args(&self.args);
        for (key, value) in self.env_vars {
            cmd.env(key, value);
        }
        // Set NO_COLOR to avoid ANSI codes in test output
        cmd.env("NO_COLOR", "1");
        cmd.assert()
    }

    pub async fn output(self) -> std::process::Output {
        let mut cmd = Command::cargo_bin("boundless").unwrap();
        cmd.args(&self.args);
        for (key, value) in self.env_vars {
            cmd.env(key, value);
        }
        cmd.env("NO_COLOR", "1");
        cmd.output().unwrap()
    }
}
