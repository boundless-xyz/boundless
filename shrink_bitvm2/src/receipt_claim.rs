use anyhow::Context;
use risc0_zkvm::{
    sha::{self, Digestible, Sha256, DIGEST_BYTES},
    Digest, MaybePruned, SystemState, VerifierContext,
};
use serde::{Deserialize, Serialize};

/// A claim about the guest program execution, such as the journal.
/// The digest of this is what the BLAKE3 Groth16 proof outputs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShrinkBitvm2ReceiptClaim {
    pub pre: MaybePruned<SystemState>,
    pub post: MaybePruned<SystemState>,
    /// Note: This journal has to be exactly 32 bytes
    pub journal: Vec<u8>,

    pub control_root: Digest,
    pub control_id: Digest,
}

impl ShrinkBitvm2ReceiptClaim {
    pub fn ok(image_id: impl Into<Digest>, journal: impl Into<Vec<u8>>) -> Self {
        Self::ok_with_ctx(image_id, journal, VerifierContext::default())
            .expect("default verifier context is expected to supply succinct verifier parameters")
    }

    pub fn ok_with_ctx(
        image_id: impl Into<Digest>,
        journal: impl Into<Vec<u8>>,
        ctx: VerifierContext,
    ) -> anyhow::Result<Self> {
        let verifier_params = ctx
            .succinct_verifier_parameters
            .as_ref()
            .context("succinct verifier parameters is required but was not provided")?;
        let control_root = verifier_params.control_root;
        Ok(Self {
            control_root,
            pre: MaybePruned::Pruned(image_id.into()),
            post: MaybePruned::Value(SystemState {
                pc: 0,
                merkle_root: Digest::ZERO,
            }),
            control_id: crate::BN254_IDENTITY_CONTROL_ID,
            journal: journal.into(),
        })
    }

    pub fn claim_digest(&self) -> Digest {
        self.claim_digest_inner::<sha::Impl>()
    }

    // Implementation of the claim digest using the provided generic implementation of
    // SHA-256. There is no known reason to use an implemention other than the one provided as
    // risc0-zkvm::sha::Impl, so this method is not exported.
    fn claim_digest_inner<S: Sha256>(&self) -> Digest {
        // Byte-order of the control root is reversed for usage in the recursive verifier circuit.
        // NOTE: In this case, the root is a Poseidon hash using BN-254 scalar field.
        let mut control_root_bytes: [u8; 32] = self.control_root.as_bytes().try_into().unwrap();
        for byte in &mut control_root_bytes {
            *byte = byte.reverse_bits();
        }

        let mut buffer = [0u8; DIGEST_BYTES * 4];
        let (buffer_chunks, &mut []) = buffer.as_chunks_mut::<DIGEST_BYTES>() else {
            unreachable!()
        };

        buffer_chunks[0] = control_root_bytes;
        buffer_chunks[1] = self.pre.digest().into();
        buffer_chunks[2] = self.post.digest().into();
        buffer_chunks[3] = self.control_id.into();

        let output_prefix = S::hash_bytes(&buffer);

        // final blake3 hash
        let mut hasher = blake3::Hasher::new();
        hasher.update(output_prefix.as_bytes());
        hasher.update(&self.journal);

        let mut digest_bytes: [u8; 32] = hasher.finalize().into();
        // trim to 31 bytes
        digest_bytes[31] = 0;
        // shift because of endianness
        digest_bytes.rotate_right(1);
        digest_bytes.into()
    }
}

impl risc0_binfmt::Digestible for ShrinkBitvm2ReceiptClaim {
    fn digest<S: Sha256>(&self) -> Digest {
        self.claim_digest_inner::<S>()
    }
}
