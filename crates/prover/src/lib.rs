use async_trait::async_trait;
use serde::{Deserialize, Serialize};

mod bonsai;
mod boundless;
mod local;

pub use bonsai::BonsaiProgram;
pub use boundless::BoundlessProgram;
pub use local::LocalProgram;

/// Output from generating a proof. This represents the data that will be posted to the contract on
/// chain.
#[derive(Debug, Serialize, Deserialize)]
pub struct ProofOutput {
    pub journal: Vec<u8>,
    pub seal: Vec<u8>,
}

#[async_trait]
pub trait AsyncProve {
    /// Prove the execution of the program given provided input.
    async fn prove(&self, input: Vec<u8>) -> anyhow::Result<ProofOutput>;
}

#[cfg(test)]
mod tests {
    use alloy::{primitives::Address, signers::local::PrivateKeySigner};
    use boundless_market::sdk::client::Client;

    use super::*;

    const TEST_ELF: &[u8] = include_bytes!("../../bento-client/method_name");

    #[tokio::test]
    #[ignore]
    async fn test_local_prove() -> anyhow::Result<()> {
        let program = LocalProgram::new(TEST_ELF);
        program.prove(vec![]).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn test_bonsai_prove() -> anyhow::Result<()> {
        let program = BonsaiProgram::upload_elf(TEST_ELF).await?;
        program.prove(vec![]).await?;
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn test_boundless_prove() -> anyhow::Result<()> {
        let boundless_client = Client::from_parts(
            PrivateKeySigner::random(),
            "https://some-url.com".parse()?,
            Address::ZERO,
            Address::ZERO,
        )
        .await?;
        let program = BoundlessProgram::upload_elf(boundless_client, TEST_ELF).await?;
        program.prove(vec![]).await?;
        Ok(())
    }
}
