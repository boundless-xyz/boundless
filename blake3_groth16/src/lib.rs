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
#[cfg(not(target_os = "zkvm"))]
use anyhow::{Context, Result};
pub use boundless_market::blake3_groth16::Blake3Groth16ReceiptClaim;
pub use receipt::*;
#[cfg(not(target_os = "zkvm"))]
pub use risc0_groth16::{ProofJson as Groth16ProofJson, Seal as Groth16Seal};
#[cfg(not(target_os = "zkvm"))]
use risc0_zkvm::{default_prover, sha::Digestible, InnerReceipt, MaybePruned, Receipt};

#[cfg(all(feature = "prove", not(target_os = "zkvm")))]
use risc0_zkvm::ProverOpts;

#[cfg(all(feature = "prove", not(target_os = "zkvm")))]
mod prove;
pub mod receipt;

pub mod verify;

#[cfg(not(target_os = "zkvm"))]
/// Compresses a Receipt into a BLAKE3 Groth16 Receipt.
pub async fn compress_blake3_groth16(receipt: &Receipt) -> Result<Blake3Groth16Receipt> {
    tracing::debug!("Compressing receipt to blake3 groth16");
    if is_dev_mode() {
        println!("RISC0_DEV_MODE is set, skipping actual blake3 groth16 compression and returning fake receipt");
        let mut receipt = receipt.clone();
        let blake3_claim_digest = Blake3Groth16ReceiptClaim::try_from(
            receipt.claim()?.value().context("receipt claim must not be pruned")?,
        )?
        .digest();
        if let InnerReceipt::Fake(fake_receipt) = &mut receipt.inner {
            fake_receipt.claim = MaybePruned::Pruned(blake3_claim_digest)
        } else {
            return Err(anyhow::anyhow!(
                "RISC0_DEV_MODE blake3_groth16 compression can only be used on fake receipts"
            ));
        }
        return receipt.try_into();
    }
    if default_prover().get_name() == "bonsai" {
        let client = bonsai_sdk::non_blocking::Client::from_env(risc0_zkvm::VERSION)?;
        tracing::info!("Using bonsai to compress to blake3 groth16");
        return compress_blake3_groth16_bonsai(&client, receipt).await;
    }
    let _receipt = receipt.clone();

    #[cfg(not(feature = "prove"))]
    {
        Err(anyhow::anyhow!(
            "blake3_groth16 must be built with the 'prove' feature to compress receipts locally"
        ))
    }

    #[cfg(feature = "prove")]
    tokio::task::spawn_blocking(move || {
        let receipt = _receipt;
        let succinct_receipt = default_prover().compress(&ProverOpts::succinct(), &receipt)?;
        tracing::debug!("Succinct receipt created, proceeding to convert to blake3 groth16");
        let receipt = succinct_receipt.clone();
        let journal: [u8; 32] = receipt
            .journal
            .bytes
            .as_slice()
            .try_into()
            .context("invalid journal length, expected 32 bytes for shrink blake3")?;
        let seal = prove::succinct_to_blake3_groth16(
            receipt
                .inner
                .succinct()
                .context("compressing to blake3 groth16 requires a succinct receipt")?,
            journal,
        )?;
        let seal: Groth16Seal = seal.try_into()?;
        let blake3_receipt = Blake3Groth16Receipt::finalize(receipt.claim()?, seal.to_vec())?;
        Ok(blake3_receipt)
    })
    .await?
}

#[cfg(not(target_os = "zkvm"))]
async fn compress_blake3_groth16_bonsai(
    client: &bonsai_sdk::non_blocking::Client,
    succinct_receipt: &Receipt,
) -> Result<Blake3Groth16Receipt> {
    let encoded_receipt = bincode::serialize(succinct_receipt)?;
    let receipt_id = client.upload_receipt(encoded_receipt).await?;
    let snark_id = client.shrink_bitvm2(receipt_id).await?;
    loop {
        let status = snark_id.status(client).await?;
        match status.status.as_ref() {
            "RUNNING" => {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                continue;
            }
            "SUCCEEDED" => {
                let receipt_buf = client.download(&status.output.unwrap()).await?;
                let snark_receipt: Blake3Groth16Receipt = bincode::deserialize(&receipt_buf)?;
                return Ok(snark_receipt);
            }
            status_code => {
                let err_msg = status.error_msg.unwrap_or_default();
                return Err(anyhow::anyhow!(
                    "snark proving failed with status {status_code}: {err_msg}"
                ));
            }
        }
    }
}

#[cfg(not(target_os = "zkvm"))]
fn is_dev_mode() -> bool {
    std::env::var("RISC0_DEV_MODE")
        .ok()
        .map(|x| x.to_lowercase())
        .filter(|x| x == "1" || x == "true" || x == "yes")
        .is_some()
}

#[cfg(all(test, not(target_os = "zkvm")))]
mod tests {
    use super::*;
    use guest_util::ECHO_ELF;
    use risc0_zkvm::{default_prover, sha::Digestible, ExecutorEnv, ProverOpts};

    #[tokio::test]
    #[test_log::test]
    async fn test_succinct_to_blake3_groth16() {
        use guest_util::ECHO_ID;

        let input = [3u8; 32];

        let env = ExecutorEnv::builder().write_slice(&input).build().unwrap();
        tracing::info!("Proving echo program to get initial receipt");
        let prover = default_prover();
        // Produce a receipt by proving the specified ELF binary.
        let receipt =
            prover.prove_with_opts(env, ECHO_ELF, &ProverOpts::succinct()).unwrap().receipt;
        tracing::info!("Initial receipt created, compressing to blake3_groth16");
        let blake3_receipt = compress_blake3_groth16(&receipt).await.unwrap();
        let blake3_claim_digest = Blake3Groth16ReceiptClaim::ok(ECHO_ID, input.to_vec()).digest();
        assert_eq!(blake3_receipt.claim_digest().unwrap(), blake3_claim_digest);
        blake3_receipt.verify(ECHO_ID).expect("verification failed");
    }
}
