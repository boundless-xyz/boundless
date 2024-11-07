use crate::{AsyncProve, ProofOutput};
use async_trait::async_trait;
use risc0_ethereum_contracts::groth16;
use risc0_zkvm::{default_prover, is_dev_mode, sha::Digestible, ExecutorEnv, ProverOpts};

/// A static ELF program that can be proven locally.
/// 
/// A proof can be generated using the [AsyncProve::prove] method.
#[derive(Debug)]
pub struct LocalProgram {
    elf: &'static [u8],
}

impl LocalProgram {
    pub fn new(elf: &'static [u8]) -> Self {
        Self { elf }
    }
}

#[async_trait]
impl AsyncProve for LocalProgram {
    async fn prove(&self, input: Vec<u8>) -> anyhow::Result<ProofOutput> {
        let elf = self.elf;
        let receipt = tokio::task::spawn_blocking(move || {
            let env = ExecutorEnv::builder().write_slice(&input).build()?;

            // TODO: Might want to disallow Bonsai from being chosen here, if intentional to be
            //       strict to avoid uploading ELF for each proof.
            let prover = default_prover();
            prover.prove_with_opts(env, elf, &ProverOpts::groth16())
        })
        .await??
        .receipt;

        let seal = match is_dev_mode() {
            true => [&[0u8; 4], receipt.claim()?.digest().as_bytes()].concat(),
            false => groth16::encode(receipt.inner.groth16()?.seal.clone())?,
        };

        Ok(ProofOutput { journal: receipt.journal.bytes, seal })
    }
}

