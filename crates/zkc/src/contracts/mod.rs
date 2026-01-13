// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

//! Smart contract interfaces and bytecode for ZKC contracts.

use std::fmt::Debug;

use alloy::{
    rpc::types::{Log, TransactionReceipt},
    sol_types::{SolEvent, SolInterface},
};
use anyhow::{anyhow, Context, Result};

alloy::sol!(
    #![sol(rpc, all_derives)]
    "src/contracts/artifacts/IZKC.sol"
);

alloy::sol!(
    #![sol(rpc, all_derives)]
    "src/contracts/artifacts/IStaking.sol"
);

alloy::sol!(
    #![sol(rpc, all_derives)]
    "src/contracts/artifacts/IRewards.sol"
);

alloy::sol!(
    #![sol(rpc, all_derives)]
    "src/contracts/artifacts/IStakingRewards.sol"
);

alloy::sol!(
    #![sol(rpc, all_derives)]
    "src/contracts/artifacts/IVotes.sol"
);

pub fn extract_tx_log<E: SolEvent + Debug + Clone>(
    receipt: &TransactionReceipt,
) -> Result<Log<E>, anyhow::Error> {
    let logs = receipt
        .inner
        .logs()
        .iter()
        .filter_map(|log| {
            if log.topic0().map(|topic| E::SIGNATURE_HASH == *topic).unwrap_or(false) {
                Some(
                    log.log_decode::<E>()
                        .with_context(|| format!("failed to decode event {}", E::SIGNATURE)),
                )
            } else {
                tracing::debug!(
                    "skipping log on receipt; does not match {}: {log:?}",
                    E::SIGNATURE
                );
                None
            }
        })
        .collect::<Result<Vec<_>>>()?;

    match &logs[..] {
        [log] => Ok(log.clone()),
        [] => Err(anyhow!(
            "transaction 0x{:x} did not emit event {}",
            receipt.transaction_hash,
            E::SIGNATURE
        )),
        _ => Err(anyhow!(
            "transaction emitted more than one event with signature {}, {:#?}",
            E::SIGNATURE,
            logs
        )),
    }
}

pub fn extract_tx_logs<E: SolEvent + Debug + Clone>(
    receipt: &TransactionReceipt,
) -> Result<Vec<Log<E>>, anyhow::Error> {
    let logs = receipt
        .inner
        .logs()
        .iter()
        .filter_map(|log| {
            if log.topic0().map(|topic| E::SIGNATURE_HASH == *topic).unwrap_or(false) {
                Some(
                    log.log_decode::<E>()
                        .with_context(|| format!("failed to decode event {}", E::SIGNATURE)),
                )
            } else {
                tracing::debug!(
                    "skipping log on receipt; does not match {}: {log:?}",
                    E::SIGNATURE
                );
                None
            }
        })
        .collect::<Result<Vec<_>>>()?;

    match &logs[..] {
        [] => Err(anyhow!(
            "transaction 0x{:x} did not emit event {}",
            receipt.transaction_hash,
            E::SIGNATURE
        )),
        _ => Ok(logs),
    }
}

pub trait DecodeRevert<T> {
    fn maybe_decode_revert<E: SolInterface + Debug>(self) -> Result<T>;
}

impl<T> DecodeRevert<T> for alloy::contract::Result<T, alloy::contract::Error> {
    fn maybe_decode_revert<E: SolInterface + Debug>(self) -> Result<T> {
        self.map_err(|err| match err.as_decoded_interface_error::<E>() {
            Some(e) => anyhow!("execution reverted with error: {e:?}"),
            None => err.into(),
        })
    }
}
