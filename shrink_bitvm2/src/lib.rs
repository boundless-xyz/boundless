use anyhow::{Context, Result};
pub use receipt_claim::*;
use risc0_circuit_recursion::control_id::BN254_IDENTITY_CONTROL_ID;
pub use risc0_groth16::{ProofJson as Groth16ProofJson, Seal as Groth16Seal};
use risc0_zkvm::{default_prover, MaybePruned, ProverOpts, Receipt, ReceiptClaim, SuccinctReceipt};

#[cfg(feature = "prove")]
use {risc0_zkvm::sha::Digestible, risc0_zkvm::Groth16Receipt, std::path::Path, tempfile::tempdir};

#[cfg(feature = "prove")]
mod prove;
pub mod receipt_claim;
pub mod verify;

/// Compresses a Receipt into a BLAKE3 Groth16 Receipt.
pub async fn compress_bitvm2(receipt: &Receipt) -> Result<Receipt> {
    tracing::debug!("Compressing receipt to shrink_bitvm2");
    if is_dev_mode() {
        return Ok(receipt.clone());
    }
    if default_prover().get_name() == "bonsai" {
        let client = bonsai_sdk::non_blocking::Client::from_env(risc0_zkvm::VERSION)?;
        tracing::info!("Using bonsai to compress to shrink_bitvm2");
        return compress_bitvm2_with_bonsai(&client, receipt).await;
    }
    let receipt = receipt.clone();
    #[cfg(not(feature = "prove"))]
    {
        Err(anyhow!(
            "shrink_bitvm2 must be built with the 'prove' feature to compress receipts locally"
        ))
    }
    #[cfg(feature = "prove")]
    tokio::task::spawn_blocking(move || {
        let succinct_receipt = default_prover().compress(&ProverOpts::succinct(), &receipt)?;
        tracing::debug!("Succinct receipt created, proceeding to convert to shrink_bitvm2");
        let receipt = succinct_receipt.clone();
        let journal: [u8; 32] = receipt
            .journal
            .bytes
            .as_slice()
            .try_into()
            .context("invalid journal length, expected 32 bytes for shrink blake3")?;
        let seal = succinct_to_bitvm2(
            receipt
                .inner
                .succinct()
                .context("compressing to blake3 groth16 requires a succinct receipt")?,
            journal,
        )?;
        finalize(journal, receipt.claim()?, &seal.try_into()?)
    })
    .await?
}

/// Creates a BLAKE3 Groth16 proof from a Risc0 SuccinctReceipt.
/// It will first run the identity_p254 program to convert the STARK to BN254,
/// which is more efficient to verify.
#[cfg(feature = "prove")]
fn succinct_to_bitvm2(
    succinct_receipt: &SuccinctReceipt<ReceiptClaim>,
    journal: [u8; 32],
) -> Result<Groth16ProofJson> {
    let p254_receipt = risc0_zkvm::get_prover_server(&ProverOpts::default())?
        .identity_p254(succinct_receipt)
        .context("failed to create p254 receipt")?;
    shrink_wrap(&p254_receipt, journal)
}

/// Creates a BLAKE3 Groth16 proof from a identity p254 Risc0 SuccinctReceipt.
#[cfg(feature = "prove")]
fn shrink_wrap(
    p254_receipt: &SuccinctReceipt<ReceiptClaim>,
    journal: [u8; 32],
) -> Result<Groth16ProofJson> {
    let seal_json = prove::identity_seal_json(journal, p254_receipt)?;

    let tmp_dir = tempdir().context("failed to create temporary directory")?;
    let work_dir = std::env::var("BLAKE3_GROTH16_WORK_DIR");
    let work_dir = work_dir.as_ref().map(Path::new).unwrap_or(tmp_dir.path());

    #[cfg(feature = "cuda")]
    let proof_json = prove::cuda::shrink_wrap(work_dir, seal_json)?;
    #[cfg(not(feature = "cuda"))]
    let proof_json = prove::rapidsnark::shrink_wrap(work_dir, seal_json)?;

    Ok(proof_json)
}

/// Verifies the BLAKE3 Groth16Seal against the BLAKE3 claim digest and wraps it in a Receipt.
#[cfg(feature = "prove")]
fn finalize(
    journal: [u8; 32],
    receipt_claim: MaybePruned<ReceiptClaim>,
    seal: &Groth16Seal,
) -> Result<Receipt> {
    let receipt_claim_value =
        receipt_claim.as_value().context("receipt claim must not be pruned")?;
    let blake3_claim_digest =
        ShrinkBitvm2ReceiptClaim::ok(receipt_claim_value.pre.digest(), journal.to_vec()).digest();
    verify::verify(seal, blake3_claim_digest)?;

    let verifier_parameters = crate::verify::verifier_parameters();
    let groth16_receipt =
        Groth16Receipt::new(seal.to_vec(), receipt_claim, verifier_parameters.digest());
    let receipt =
        Receipt::new(risc0_zkvm::InnerReceipt::Groth16(groth16_receipt), journal.to_vec());
    Ok(receipt)
}

async fn compress_bitvm2_with_bonsai(
    client: &bonsai_sdk::non_blocking::Client,
    succinct_receipt: &Receipt,
) -> Result<Receipt> {
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
                let snark_receipt: Receipt = bincode::deserialize(&receipt_buf)?;
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

fn is_dev_mode() -> bool {
    std::env::var("RISC0_DEV_MODE")
        .ok()
        .map(|x| x.to_lowercase())
        .filter(|x| x == "1" || x == "true" || x == "yes")
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_util::ECHO_ELF;
    use risc0_zkvm::{default_prover, ExecutorEnv, ProverOpts};

    #[tokio::test]
    #[test_log::test]
    async fn test_succinct_to_bitvm2() {
        use guest_util::ECHO_ID;

        let input = [3u8; 32];

        let env = ExecutorEnv::builder().write_slice(&input).build().unwrap();
        tracing::info!("Proving echo program to get initial receipt");
        let prover = default_prover();
        // Produce a receipt by proving the specified ELF binary.
        let receipt =
            prover.prove_with_opts(env, ECHO_ELF, &ProverOpts::succinct()).unwrap().receipt;
        tracing::info!("Initial receipt created, compressing to shrink_bitvm2");
        let groth16_receipt = compress_bitvm2(&receipt).await.unwrap();
        let groth16_receipt = groth16_receipt.inner.groth16().unwrap();
        let blake3_g16_seal = Groth16Seal::decode(&groth16_receipt.seal).unwrap();
        let blake3_claim_digest = ShrinkBitvm2ReceiptClaim::ok(ECHO_ID, input.to_vec()).digest();
        verify::verify(&blake3_g16_seal, blake3_claim_digest).expect("verification failed");
    }
}
