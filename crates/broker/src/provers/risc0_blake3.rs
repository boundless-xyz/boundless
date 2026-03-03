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

use async_trait::async_trait;
use blake3_groth16::Blake3Groth16Receipt;

use super::{Prover, ProverError};

macro_rules! delegate_prover {
    ($wrapper:ty) => {
        #[async_trait]
        impl Prover for $wrapper {
            async fn has_image(&self, image_id: &str) -> Result<bool, ProverError> {
                self.inner.has_image(image_id).await
            }
            async fn upload_input(&self, input: Vec<u8>) -> Result<String, ProverError> {
                self.inner.upload_input(input).await
            }
            async fn upload_image(
                &self,
                image_id: &str,
                image: Vec<u8>,
            ) -> Result<(), ProverError> {
                self.inner.upload_image(image_id, image).await
            }
            async fn preflight(
                &self,
                image_id: &str,
                input_id: &str,
                assumptions: Vec<String>,
                executor_limit: Option<u64>,
                order_id: &str,
            ) -> Result<super::ProofResult, ProverError> {
                self.inner
                    .preflight(image_id, input_id, assumptions, executor_limit, order_id)
                    .await
            }
            async fn prove_stark(
                &self,
                image_id: &str,
                input_id: &str,
                assumptions: Vec<String>,
            ) -> Result<String, ProverError> {
                self.inner.prove_stark(image_id, input_id, assumptions).await
            }
            async fn wait_for_stark(
                &self,
                proof_id: &str,
            ) -> Result<super::ProofResult, ProverError> {
                self.inner.wait_for_stark(proof_id).await
            }
            async fn cancel_stark(&self, proof_id: &str) -> Result<(), ProverError> {
                self.inner.cancel_stark(proof_id).await
            }
            async fn get_receipt(
                &self,
                proof_id: &str,
            ) -> Result<Option<super::ProverReceipt>, ProverError> {
                self.inner.get_receipt(proof_id).await
            }
            async fn get_preflight_journal(
                &self,
                proof_id: &str,
            ) -> Result<Option<Vec<u8>>, ProverError> {
                self.inner.get_preflight_journal(proof_id).await
            }
            async fn get_journal(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError> {
                self.inner.get_journal(proof_id).await
            }
            async fn compute_image_id(
                &self,
                elf: &[u8],
            ) -> Result<risc0_zkvm::sha::Digest, ProverError> {
                self.inner.compute_image_id(elf).await
            }
            async fn compress(&self, proof_id: &str) -> Result<String, ProverError> {
                self.inner.compress_blake3_groth16(proof_id).await
            }
            async fn get_compressed_receipt(
                &self,
                proof_id: &str,
            ) -> Result<Option<Vec<u8>>, ProverError> {
                self.inner.get_blake3_groth16_receipt(proof_id).await
            }
            async fn encode_compressed_seal(&self, proof_id: &str) -> Result<Vec<u8>, ProverError> {
                encode_blake3_groth16_seal(self, proof_id).await
            }
            async fn verify_compressed_receipt(&self, proof_id: &str) -> Result<(), ProverError> {
                verify_blake3_groth16_receipt(self, proof_id).await
            }
        }
    };
}

async fn encode_blake3_groth16_seal(
    prover: &impl Prover,
    proof_id: &str,
) -> Result<Vec<u8>, ProverError> {
    let receipt_bytes = prover.get_compressed_receipt(proof_id).await?.ok_or_else(|| {
        ProverError::NotFound(format!("Blake3 Groth16 receipt not found: {proof_id}"))
    })?;

    let receipt: Blake3Groth16Receipt = bincode::deserialize(&receipt_bytes).map_err(|e| {
        ProverError::ProverInternalError(format!("Failed to deserialize receipt: {e}"))
    })?;

    risc0_ethereum_contracts::encode_seal(&receipt.into()).map_err(|e| {
        ProverError::ProverInternalError(format!("Failed to encode Blake3 Groth16 seal: {e}"))
    })
}

async fn verify_blake3_groth16_receipt(
    prover: &impl Prover,
    proof_id: &str,
) -> Result<(), ProverError> {
    let receipt_bytes = prover.get_compressed_receipt(proof_id).await?.ok_or_else(|| {
        ProverError::NotFound(format!("Blake3 Groth16 receipt not found: {proof_id}"))
    })?;

    let receipt: Blake3Groth16Receipt = bincode::deserialize(&receipt_bytes).map_err(|e| {
        ProverError::ProverInternalError(format!("Failed to deserialize receipt: {e}"))
    })?;

    receipt.verify_integrity().map_err(|e| {
        ProverError::ProverInternalError(format!("Blake3 Groth16 verification failed: {e}"))
    })?;

    Ok(())
}

pub struct Blake3DefaultProver {
    inner: std::sync::Arc<super::DefaultProver>,
}

impl Blake3DefaultProver {
    pub fn new(inner: std::sync::Arc<super::DefaultProver>) -> Self {
        Self { inner }
    }
}

delegate_prover!(Blake3DefaultProver);

pub struct Blake3Bonsai {
    inner: std::sync::Arc<super::Bonsai>,
}

impl Blake3Bonsai {
    pub fn new(inner: std::sync::Arc<super::Bonsai>) -> Self {
        Self { inner }
    }
}

delegate_prover!(Blake3Bonsai);

#[cfg(test)]
mod tests {
    use super::super::DefaultProver;
    use super::*;
    use boundless_test_utils::guests::{ECHO_ELF, ECHO_ID};
    use risc0_zkvm::sha::Digest;

    #[tokio::test]
    async fn test_blake3_default_prover_compress() {
        let inner = std::sync::Arc::new(DefaultProver::new());
        let image_id = Digest::from(ECHO_ID);
        inner.upload_image(&image_id.to_string(), ECHO_ELF.to_vec()).await.unwrap();

        let input_data = [255u8; 32].to_vec();
        let input_id = inner.upload_input(input_data.clone()).await.unwrap();

        let result =
            inner.prove_and_monitor_stark(&image_id.to_string(), &input_id, vec![]).await.unwrap();

        let blake3_prover = Blake3DefaultProver::new(inner);
        let snark_id = blake3_prover.compress(&result.id).await.unwrap();

        // Verify via the wrapper's verify_compressed_receipt
        blake3_prover.verify_compressed_receipt(&snark_id).await.unwrap();

        // Also verify manually
        let compressed_receipt =
            blake3_prover.get_compressed_receipt(&snark_id).await.unwrap().unwrap();
        let blake3_receipt: blake3_groth16::Blake3Groth16Receipt =
            bincode::deserialize(&compressed_receipt).unwrap();
        blake3_receipt.verify(ECHO_ID).expect("blake3 groth16 verification failed");
    }
}
