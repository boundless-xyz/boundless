use anyhow::Result;
pub use receipt_claim::*;
use risc0_circuit_recursion::control_id::BN254_IDENTITY_CONTROL_ID;
pub use risc0_groth16::{ProofJson as Groth16ProofJson, Seal as Groth16Seal};
use risc0_zkvm::{MaybePruned, Receipt, ReceiptClaim, SuccinctReceipt};

#[cfg(feature = "prove")]
use {
    anyhow::Context, risc0_zkvm::Groth16Receipt, risc0_zkvm::sha::Digestible, std::path::Path,
    tempfile::tempdir,
};

#[cfg(feature = "prove")]
mod prove;
pub mod receipt_claim;
pub mod verify;

/// Creates a BLAKE3 Groth16 proof from a Risc0 SuccinctReceipt.
/// It will first run the identity_p254 program to convert the STARK to BN254,
/// which is more efficient to verify.
#[cfg(feature = "prove")]
pub fn succinct_to_bitvm2(
    succinct_receipt: &SuccinctReceipt<ReceiptClaim>,
    journal: [u8; 32],
) -> Result<Groth16ProofJson> {
    let p254_receipt: SuccinctReceipt<ReceiptClaim> =
        risc0_zkvm::recursion::identity_p254(succinct_receipt).unwrap();
    shrink_wrap(&p254_receipt, journal)
}

/// Creates a BLAKE3 Groth16 proof from a Risc0 SuccinctReceipt.
#[cfg(feature = "prove")]
pub fn shrink_wrap(
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
pub fn finalize(
    journal: [u8; 32],
    receipt_claim: MaybePruned<ReceiptClaim>,
    seal: &Groth16Seal,
) -> Result<Receipt> {
    let receipt_claim_value = receipt_claim
        .as_value()
        .context("receipt claim must not be pruned")?;
    let blake3_claim_digest =
        ShrinkBitvm2ReceiptClaim::ok(receipt_claim_value.pre.digest(), journal.to_vec()).digest();
    verify::verify(seal, blake3_claim_digest)?;

    let verifier_parameters = crate::verify::verifier_parameters();
    let groth16_receipt =
        Groth16Receipt::new(seal.to_vec(), receipt_claim, verifier_parameters.digest());
    let receipt = Receipt::new(
        risc0_zkvm::InnerReceipt::Groth16(groth16_receipt),
        journal.to_vec(),
    );
    Ok(receipt)
}

#[cfg(not(feature = "prove"))]
pub fn succinct_to_bitvm2(
    _succinct_receipt: &SuccinctReceipt<ReceiptClaim>,
    _journal: [u8; 32],
) -> Result<Groth16ProofJson> {
    unimplemented!(
        "shrink_bitvm2 must be built with the 'prove' feature to convert a SuccinctReceipt to a ShrinkBitvm2 Receipt"
    );
}

#[cfg(not(feature = "prove"))]
pub fn shrink_wrap(
    _p254_receipt: &SuccinctReceipt<ReceiptClaim>,
    _journal: [u8; 32],
) -> Result<Groth16ProofJson> {
    unimplemented!(
        "shrink_bitvm2 must be built with the 'prove' feature to convert a SuccinctReceipt to a ShrinkBitvm2 Receipt"
    );
}

#[cfg(not(feature = "prove"))]
pub fn finalize(
    _journal_bytes: [u8; 32],
    _receipt_claim: MaybePruned<ReceiptClaim>,
    _seal: &Groth16Seal,
) -> Result<Receipt> {
    unimplemented!(
        "shrink_bitvm2 must be built with the 'prove' feature to convert a SuccinctReceipt to a ShrinkBitvm2 Receipt"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest::ECHO_ELF;
    #[cfg(feature = "prove")]
    use risc0_zkvm::{ExecutorEnv, ProverOpts, default_prover};
    #[cfg(feature = "prove")]
    #[test]
    fn test_succinct_to_bitvm2() {
        use guest::ECHO_ID;

        let input = [3u8; 32];

        let env = ExecutorEnv::builder().write_slice(&input).build().unwrap();

        // Obtain the default prover.
        let prover = default_prover();

        // Produce a receipt by proving the specified ELF binary.
        let receipt = prover
            .prove_with_opts(env, ECHO_ELF, &ProverOpts::succinct())
            .unwrap()
            .receipt;
        let succinct_receipt = receipt.inner.succinct().unwrap();

        let blake3_g16_seal_json = succinct_to_bitvm2(succinct_receipt, input).unwrap();
        let blake3_claim_digest = ShrinkBitvm2ReceiptClaim::ok(ECHO_ID, input.to_vec()).digest();
        let blake3_g16_seal: Groth16Seal = blake3_g16_seal_json.try_into().unwrap();
        verify::verify(&blake3_g16_seal, blake3_claim_digest).expect("verification failed");

        let _ =
            finalize(input, receipt.claim().unwrap(), &blake3_g16_seal).expect("finalize failed");
    }
}
