use alloy::hex;
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use bonsai_sdk::non_blocking::Client;
use risc0_ethereum_contracts::groth16;
use risc0_zkvm::{compute_image_id, sha::Digest, Receipt};
use std::time::Duration;

use crate::{AsyncProve, ProofOutput};

/// An ELF program uploaded to Bonsai, ready to be proven.
///
/// A proof can be generated using the [AsyncProve::prove] method.
pub struct BonsaiProgram {
    deployed_id: Digest,
}

impl BonsaiProgram {
    /// Upload an ELF binary to Bonsai. This only needs to be one once per ELF.
    pub async fn upload_elf(elf: impl Into<Vec<u8>>) -> Result<Self> {
        let client = Client::from_env(risc0_zkvm::VERSION)?;
        let elf = elf.into();

        let image_id = compute_image_id(&elf)?;
        let image_id_hex = hex::encode(image_id);
        client.upload_img(&image_id_hex, elf).await?;
        Ok(Self { deployed_id: image_id })
    }
}

#[async_trait]
impl AsyncProve for BonsaiProgram {
    async fn prove(&self, input: Vec<u8>) -> anyhow::Result<ProofOutput> {
        let client = Client::from_env(risc0_zkvm::VERSION)?;

        let polling_interval = polling_interval()?;
        let image_id = self.deployed_id;
        // upload input data
        let input_id = client.upload_input(input).await?;

        // upload receipts
        let receipts_ids: Vec<String> = vec![];
        // for assumption in &env.assumptions.borrow().cached {
        //     let serialized_receipt = match assumption {
        //         AssumptionReceipt::Proven(receipt) => bincode::serialize(&receipt)?,
        //         AssumptionReceipt::Unresolved(_) => {
        //             bail!("only proven assumptions can be uploaded to Bonsai.")
        //         }
        //     };
        //     // TODO: this can be parallelized.
        //     let receipt_id = client.upload_receipt(serialized_receipt).await?;
        //     receipts_ids.push(receipt_id);
        // }

        // While this is the executor, we want to start a session on the bonsai prover.
        // By doing so, we can return a session ID so that the prover can use it to
        // retrieve the receipt.
        let session =
            client.create_session(hex::encode(image_id), input_id, receipts_ids, false).await?;
        tracing::debug!("Bonsai proving SessionID: {}", session.uuid);

        loop {
            // The session has already been started in the executor. Poll bonsai to check if
            // the proof request succeeded.
            let res = session.status(&client).await?;
            if res.status == "RUNNING" {
                tokio::time::sleep(polling_interval).await;
                continue;
            }
            if res.status == "SUCCEEDED" {
                break;
            } else {
                bail!(
                    "Bonsai prover workflow [{}] exited: {} err: {}",
                    session.uuid,
                    res.status,
                    res.error_msg.unwrap_or("Bonsai workflow missing error_msg".into()),
                );
            }
        }

        // Request that Bonsai compress further, to Groth16.
        let snark_session = client.create_snark(session.uuid).await?;
        let snark_receipt_url = loop {
            let res = snark_session.status(&client).await?;
            match res.status.as_str() {
                "RUNNING" => {
                    tokio::time::sleep(polling_interval).await;
                    continue;
                }
                "SUCCEEDED" => {
                    break res.output.with_context(|| {
                        format!(
                            "Bonsai prover workflow [{}] reported success, but provided no receipt",
                            snark_session.uuid
                        )
                    })?;
                }
                _ => {
                    bail!(
                        "Bonsai prover workflow [{}] exited: {} err: {}",
                        snark_session.uuid,
                        res.status,
                        res.error_msg.unwrap_or("Bonsai workflow missing error_msg".into()),
                    );
                }
            }
        };

        let receipt_buf = client
            .download(&snark_receipt_url)
            .await
            .with_context(|| "failed to download snark")?;
        let groth16_receipt: Receipt = bincode::deserialize(&receipt_buf)?;
        groth16_receipt
            .verify_integrity_with_context(&Default::default())
            .context("failed to verify Groth16Receipt returned by Bonsai")?;

        let seal = groth16::encode(groth16_receipt.inner.groth16()?.seal.clone())?;
        Ok(ProofOutput { journal: groth16_receipt.journal.bytes, seal })
    }
}

fn polling_interval() -> Result<Duration> {
    let polling_interval = if let Ok(ms) = std::env::var("BONSAI_POLL_INTERVAL_MS") {
        Duration::from_millis(ms.parse().context("invalid bonsai poll interval")?)
    } else {
        Duration::from_secs(1)
    };
    Ok(polling_interval)
}
