// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

//! Shared library for the Log Updater guest between guest and host.

use alloy_primitives::{Address, Signature, B256};
use alloy_sol_types::{eip712_domain, sol, Eip712Domain, SolStruct};
use anyhow::bail;

use borsh::{BorshDeserialize, BorshSerialize};
// Re-export types from risc0_povw for use in the log updater guest.
use derive_builder::Builder;
pub use risc0_povw::guest::{Journal as LogBuilderJournal, RISC0_POVW_LOG_BUILDER_ID};
use ruint::aliases::U160;
use serde::{Deserialize, Serialize};

#[cfg(feature = "build-guest")]
pub use crate::guest_artifacts::BOUNDLESS_POVW_LOG_UPDATER_PATH;
pub use crate::guest_artifacts::{BOUNDLESS_POVW_LOG_UPDATER_ELF, BOUNDLESS_POVW_LOG_UPDATER_ID};

#[cfg(feature = "host")]
sol!(
    #[sol(extra_derives(Debug, Serialize, Deserialize), rpc)]
    "./src/contracts/artifacts/IPovwAccounting.sol"
);
#[cfg(not(feature = "host"))]
sol!(
    #[sol(extra_derives(Debug, Serialize, Deserialize))]
    "./src/contracts/artifacts/IPovwAccounting.sol"
);

impl WorkLogUpdate {
    pub fn from_log_builder_journal(journal: LogBuilderJournal, value_recipient: Address) -> Self {
        Self {
            workLogId: journal.work_log_id.into(),
            initialCommit: <[u8; 32]>::from(journal.initial_commit).into(),
            updatedCommit: <[u8; 32]>::from(journal.updated_commit).into(),
            updateValue: journal.update_value,
            valueRecipient: value_recipient,
        }
    }

    pub fn eip712_domain(contract_addr: Address, chain_id: u64) -> Eip712Domain {
        eip712_domain! {
            name: "PovwAccounting",
            version: "1",
            chain_id: chain_id,
            verifying_contract: contract_addr,
        }
    }

    /// Returns the EIP-712 signing hash for the [WorkLogUpdate].
    pub fn signing_hash(&self, contract_addr: Address, chain_id: u64) -> B256 {
        self.eip712_signing_hash(&Self::eip712_domain(contract_addr, chain_id))
    }

    /// Signs the request with the given signer and EIP-712 domain derived from the given
    /// contract address and chain ID.
    #[cfg(feature = "signer")]
    pub async fn sign(
        &self,
        signer: &impl alloy_signer::Signer,
        contract_addr: Address,
        chain_id: u64,
    ) -> Result<Signature, alloy_signer::Error> {
        signer.sign_hash(&self.signing_hash(contract_addr, chain_id)).await
    }

    /// Verifies the [WorkLogUpdate] signature with the given signer and EIP-712 domain derived
    /// from the given contract address and chain ID.
    pub fn verify_signature(
        &self,
        signer: Address,
        signature: impl AsRef<[u8]>,
        contract_addr: Address,
        chain_id: u64,
    ) -> anyhow::Result<()> {
        let sig = Signature::try_from(signature.as_ref())?;
        // Check if the signature is non-canonical.
        if sig.normalize_s().is_some() {
            bail!("invalid signature: not normalized s-value");
        }
        let addr = sig.recover_address_from_prehash(&self.signing_hash(contract_addr, chain_id))?;
        if addr == signer {
            Ok(())
        } else {
            bail!("recovered signer does not match expected: {addr} != {signer}")
        }
    }
}

#[non_exhaustive]
#[derive(Builder, Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Input {
    /// Work log update built by the log builder guest.
    ///
    /// This update is verified and used to construct the [WorkLogUpdate] sent to the PoVW
    /// accounting smart contract by the log updater guest.
    pub update: LogBuilderJournal,

    /// Address that will receive any value associated with this update.
    ///
    /// The issuance of value to this address is authorized by holder of the key associated with
    /// the work log ID.
    #[borsh(
        deserialize_with = "borsh_deserialize_address",
        serialize_with = "borsh_serialize_address"
    )]
    #[builder(setter(into))]
    pub value_recipient: Address,

    /// EIP-712 ECDSA signature using the private key associated with the work log ID.
    ///
    /// This signature is verified by the log updater guest to authorize the update. Authorization
    /// is required to avoid third-parties posting conflicting updates to any given work log.
    #[builder(setter(into))]
    pub signature: Vec<u8>,

    /// Address of the PoVW accounting contract, used to form the EIP-712 domain.
    #[borsh(
        deserialize_with = "borsh_deserialize_address",
        serialize_with = "borsh_serialize_address"
    )]
    #[builder(setter(into))]
    pub contract_address: Address,

    /// EIP-155 chain ID, used to form the EIP-712 domain.
    pub chain_id: u64,
}

impl InputBuilder {
    #[cfg(feature = "signer")]
    pub async fn sign_and_build(
        &mut self,
        signer: &impl alloy_signer::Signer,
    ) -> anyhow::Result<Input> {
        use anyhow::ensure;
        use derive_builder::UninitializedFieldError;

        ensure!(self.signature.is_none(), "Cannot sign input, input already has a signature");

        let update = self.update.clone().ok_or(UninitializedFieldError::new("update"))?;
        let contract_address =
            self.contract_address.ok_or(UninitializedFieldError::new("contract_address"))?;
        let chain_id = self.chain_id.ok_or(UninitializedFieldError::new("chain_id"))?;
        ensure!(
            signer.address() == Address::from(update.work_log_id),
            "Signer does not match work log ID: signer: {}, log: {:x}",
            signer.address(),
            update.work_log_id
        );

        // Get the value recipient or set it to be equal to the log ID.
        let value_recipient = *self.value_recipient.get_or_insert(signer.address());

        self.signature = WorkLogUpdate::from_log_builder_journal(update.clone(), value_recipient)
            .sign(signer, contract_address, chain_id)
            .await?
            .as_bytes()
            .to_vec()
            .into();

        self.build().map_err(Into::into)
    }
}

impl Input {
    /// Create an [InputBuilder] to construct an [Input].
    pub fn builder() -> InputBuilder {
        Default::default()
    }

    /// Serialize the input to a vector of bytes.
    pub fn encode(&self) -> anyhow::Result<Vec<u8>> {
        borsh::to_vec(self).map_err(Into::into)
    }

    /// Deserialize the input from a slice of bytes.
    pub fn decode(buffer: impl AsRef<[u8]>) -> anyhow::Result<Self> {
        borsh::from_slice(buffer.as_ref()).map_err(Into::into)
    }
}

fn borsh_deserialize_address(
    reader: &mut impl borsh::io::Read,
) -> Result<Address, borsh::io::Error> {
    Ok(<U160 as BorshDeserialize>::deserialize_reader(reader)?.into())
}

fn borsh_serialize_address(
    address: &Address,
    writer: &mut impl borsh::io::Write,
) -> Result<(), borsh::io::Error> {
    <U160 as BorshSerialize>::serialize(&(*address).into(), writer)?;
    Ok(())
}

#[cfg(feature = "host")]
mod host {
    use std::marker::PhantomData;

    use alloy_contract::CallBuilder;
    use alloy_provider::Provider;
    use alloy_sol_types::SolValue;
    use anyhow::Context;
    use risc0_zkvm::Receipt;

    use crate::log_updater::{
        IPovwAccounting::{updateWorkLogCall, IPovwAccountingInstance},
        Journal,
    };

    impl<P: Provider> IPovwAccountingInstance<P> {
        /// Create a call to the [IPovwAccounting::updateWorkLog] function to be sent in a tx.
        pub fn update_work_log(
            &self,
            receipt: &Receipt,
        ) -> anyhow::Result<CallBuilder<&P, PhantomData<updateWorkLogCall>>> {
            let journal = Journal::abi_decode(&receipt.journal.bytes)
                .context("Failed to decode journal from Log Updater receipt")?;
            let seal = risc0_ethereum_contracts::encode_seal(receipt)
                .context("Failed to encode seal for log update")?;

            Ok(self.updateWorkLog(
                journal.update.workLogId,
                journal.update.updatedCommit,
                journal.update.updateValue,
                journal.update.valueRecipient,
                seal.into(),
            ))
        }
    }
}

#[cfg(feature = "prover")]
pub mod prover {
    use std::{borrow::Cow, convert::Infallible};

    use anyhow::Context;
    use derive_builder::Builder;
    use risc0_zkvm::{
        compute_image_id, Digest, ExecutorEnv, ProveInfo, Prover, ProverOpts, Receipt,
        VerifierContext,
    };

    use super::{
        Input, LogBuilderJournal, BOUNDLESS_POVW_LOG_UPDATER_ELF, BOUNDLESS_POVW_LOG_UPDATER_ID,
    };
    use alloy_primitives::Address;

    /// A prover for log updates which runs the Log Updater to produce a receipt for
    /// updating the PoVW accounting smart contract.
    #[derive(Builder)]
    #[builder(pattern = "owned")]
    #[non_exhaustive]
    pub struct LogUpdaterProver<P> {
        /// The underlying RISC Zero zkVM [Prover].
        #[builder(setter(custom))]
        pub prover: P,
        /// Address of the PoVW accounting contract.
        #[builder(setter(into))]
        pub contract_address: Address,
        /// Address that should receive any associated PoVW rewards.
        #[builder(setter(into), default)]
        pub value_recipient: Option<Address>,
        /// EIP-155 chain ID.
        pub chain_id: u64,
        /// Image ID for the Log Updater program.
        ///
        /// Defaults to the Log Updater program ID that is built into this crate.
        #[builder(setter(custom), default = "BOUNDLESS_POVW_LOG_UPDATER_ID.into()")]
        pub log_updater_id: Digest,
        /// Executable for the Log Updater program.
        ///
        /// Defaults to the Log Updater program that is built into this crate.
        #[builder(setter(custom), default = "BOUNDLESS_POVW_LOG_UPDATER_ELF.into()")]
        pub log_updater_program: Cow<'static, [u8]>,
        /// [ProverOpts] to use when proving the log update.
        #[builder(default)]
        pub prover_opts: ProverOpts,
        /// [VerifierContext] to use when proving the log update. This only needs to be set when using
        /// non-standard verifier parameters.
        #[builder(default)]
        pub verifier_ctx: VerifierContext,
    }

    impl<P> LogUpdaterProverBuilder<P> {
        /// Set the underlying RISC Zero zkVM [Prover].
        pub fn prover<Q>(self, prover: Q) -> LogUpdaterProverBuilder<Q> {
            LogUpdaterProverBuilder {
                prover: Some(prover),
                contract_address: self.contract_address,
                value_recipient: self.value_recipient,
                chain_id: self.chain_id,
                log_updater_id: self.log_updater_id,
                log_updater_program: self.log_updater_program,
                prover_opts: self.prover_opts,
                verifier_ctx: self.verifier_ctx,
            }
        }

        /// Set the Log Updater program, returning error if the image ID cannot be calculated.
        pub fn log_updater_program(
            self,
            program: impl Into<Cow<'static, [u8]>>,
        ) -> anyhow::Result<Self> {
            let program = program.into();
            let image_id = compute_image_id(&program)
                .context("Failed to compute image ID for Log Updater program")?;

            Ok(Self { log_updater_program: Some(program), log_updater_id: Some(image_id), ..self })
        }
    }

    impl LogUpdaterProver<Infallible> {
        /// Create a new builder for [LogUpdaterProver].
        pub fn builder() -> LogUpdaterProverBuilder<Infallible> {
            Default::default()
        }
    }

    impl<P: Prover> LogUpdaterProver<P> {
        /// Update the log and produce a proof by running the Log Updater.
        ///
        /// Takes a receipt from the Log Builder and a signer, creates the appropriate
        /// EIP-712 signature, and produces a proof for smart contract verification.
        pub async fn prove_update(
            &self,
            log_builder_receipt: Receipt,
            signer: &impl alloy_signer::Signer,
        ) -> anyhow::Result<ProveInfo> {
            // Decode the LogBuilderJournal from the receipt
            let log_builder_journal = LogBuilderJournal::decode(&log_builder_receipt.journal.bytes)
                .context("failed to deserialize LogBuilderJournal from receipt")?;

            // Create the input using the builder pattern with sign_and_build
            let input = Input::builder()
                .update(log_builder_journal)
                .value_recipient(self.value_recipient.unwrap_or(signer.address()))
                .contract_address(self.contract_address)
                .chain_id(self.chain_id)
                .sign_and_build(signer)
                .await
                .context("failed to create signed input")?;

            // Build the executor environment with the log builder receipt as an assumption
            let env = ExecutorEnv::builder()
                .write_frame(&input.encode()?)
                .add_assumption(log_builder_receipt)
                .build()
                .context("failed to build ExecutorEnv")?;

            // Prove the log update
            // NOTE: This may block the current thread for a significant amount of time. It is not
            // trivial to wrap this statement in e.g. tokio's spawn_blocking because self contains
            // a VerifierContext which does not implement Send. Using tokio block_in_place somewhat
            // mitigates the issue, but not fully.
            let prove_info = tokio::task::block_in_place(|| {
                self.prover
                    .prove_with_ctx(
                        env,
                        &self.verifier_ctx,
                        &self.log_updater_program,
                        &self.prover_opts,
                    )
                    .context("failed to prove log update")
            })?;

            Ok(prove_info)
        }
    }
}

#[cfg(test)]
mod tests {
    use risc0_zkvm::compute_image_id;

    use super::{BOUNDLESS_POVW_LOG_UPDATER_ELF, BOUNDLESS_POVW_LOG_UPDATER_ID};

    #[test]
    fn image_id_consistency() {
        assert_eq!(
            BOUNDLESS_POVW_LOG_UPDATER_ID,
            <[u32; 8]>::from(compute_image_id(BOUNDLESS_POVW_LOG_UPDATER_ELF).unwrap())
        );
    }
}
