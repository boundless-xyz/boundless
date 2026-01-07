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

//! Serialization helpers for cache storage.

use alloy::primitives::B256;
use serde::{Deserialize, Serialize};

use crate::db::TxMetadata;

/// Serializable version of TxMetadata for cache storage.
#[derive(Serialize, Deserialize)]
pub struct SerializableTxMetadata {
    pub tx_hash: Vec<u8>,
    pub from: Vec<u8>,
    pub block_number: u64,
    pub block_timestamp: u64,
    #[serde(default)]
    pub transaction_index: u64,
}

impl From<&TxMetadata> for SerializableTxMetadata {
    fn from(meta: &TxMetadata) -> Self {
        Self {
            tx_hash: meta.tx_hash.to_vec(),
            from: meta.from.to_vec(),
            block_number: meta.block_number,
            block_timestamp: meta.block_timestamp,
            transaction_index: meta.transaction_index,
        }
    }
}

impl From<SerializableTxMetadata> for TxMetadata {
    fn from(meta: SerializableTxMetadata) -> Self {
        use alloy::primitives::Address;
        Self {
            tx_hash: B256::from_slice(&meta.tx_hash),
            from: Address::from_slice(&meta.from),
            block_number: meta.block_number,
            block_timestamp: meta.block_timestamp,
            transaction_index: meta.transaction_index,
        }
    }
}
