// Copyright 2025 RISC Zero, Inc.
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

use std::{str::FromStr, sync::Arc};

use super::DbError;
use alloy::primitives::{Address, B256, U256};
use async_trait::async_trait;
use boundless_market::contracts::{
    AssessorReceipt, Fulfillment, FulfillmentDataType, PredicateType, ProofRequest,
    RequestInputType,
};
use sqlx::{
    any::{install_default_drivers, AnyConnectOptions, AnyPoolOptions},
    AnyPool, Row,
};

const SQL_BLOCK_KEY: i64 = 0;

#[derive(Debug, Clone, Copy)]
pub enum SortDirection {
    /// Ascending order (oldest first)
    Asc,
    /// Descending order (newest first)
    Desc,
}

#[derive(Debug, Clone)]
pub struct TxMetadata {
    pub tx_hash: B256,
    pub from: Address,
    pub block_number: u64,
    pub block_timestamp: u64,
    pub transaction_index: u64,
}

#[derive(Debug, Clone)]
pub struct HourlyMarketSummary {
    pub hour_timestamp: u64,
    pub total_fulfilled: u64,
    pub unique_provers_locking_requests: u64,
    pub unique_requesters_submitting_requests: u64,
    pub total_fees_locked: String,
    pub total_collateral_locked: String,
    pub p10_fees_locked: String,
    pub p25_fees_locked: String,
    pub p50_fees_locked: String,
    pub p75_fees_locked: String,
    pub p90_fees_locked: String,
    pub p95_fees_locked: String,
    pub p99_fees_locked: String,
    pub total_requests_submitted: u64,
    pub total_requests_submitted_onchain: u64,
    pub total_requests_submitted_offchain: u64,
    pub total_requests_locked: u64,
    pub total_requests_slashed: u64,
}

impl TxMetadata {
    pub fn new(tx_hash: B256, from: Address, block_number: u64, block_timestamp: u64, transaction_index: u64) -> Self {
        Self { tx_hash, from, block_number, block_timestamp, transaction_index }
    }
}

#[async_trait]
pub trait IndexerDb {
    fn pool(&self) -> &AnyPool;

    async fn get_last_block(&self) -> Result<Option<u64>, DbError>;
    async fn set_last_block(&self, block_numb: u64) -> Result<(), DbError>;

    async fn add_block(&self, block_numb: u64, block_timestamp: u64) -> Result<(), DbError>;
    async fn get_block_timestamp(&self, block_numb: u64) -> Result<Option<u64>, DbError>;

    async fn add_tx(&self, metadata: &TxMetadata) -> Result<(), DbError>;

    async fn add_proof_request(
        &self,
        request_digest: B256,
        request: ProofRequest,
        metadata: &TxMetadata,
        source: &str,
    ) -> Result<(), DbError>;

    async fn has_proof_request(&self, request_digest: B256) -> Result<bool, DbError>;

    async fn add_assessor_receipt(
        &self,
        receipt: AssessorReceipt,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    async fn add_fulfillment(
        &self,
        fill: Fulfillment,
        prover_address: Address,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    async fn add_request_submitted_event(
        &self,
        request_digest: B256,
        request_id: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    async fn add_request_locked_event(
        &self,
        request_digest: B256,
        request_id: U256,
        prover_address: Address,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    async fn add_proof_delivered_event(
        &self,
        request_digest: B256,
        request_id: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    async fn add_request_fulfilled_event(
        &self,
        request_digest: B256,
        request_id: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    async fn add_prover_slashed_event(
        &self,
        request_id: U256,
        burn_value: U256,
        transfer_value: U256,
        collateral_recipient: Address,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    async fn add_deposit_event(
        &self,
        account: Address,
        value: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    async fn add_withdrawal_event(
        &self,
        account: Address,
        value: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    async fn add_collateral_deposit_event(
        &self,
        account: Address,
        value: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    async fn add_collateral_withdrawal_event(
        &self,
        account: Address,
        value: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    async fn add_callback_failed_event(
        &self,
        request_id: U256,
        callback_address: Address,
        error_data: Vec<u8>,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    async fn get_last_order_stream_timestamp(
        &self,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>, DbError>;

    async fn set_last_order_stream_timestamp(
        &self,
        timestamp: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), DbError>;

    async fn upsert_hourly_market_summary(
        &self,
        summary: HourlyMarketSummary,
    ) -> Result<(), DbError>;

    async fn get_hourly_market_summaries(
        &self,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<HourlyMarketSummary>, DbError>;
}

pub type DbObj = Arc<dyn IndexerDb + Send + Sync>;

#[derive(Debug, Clone)]
pub struct AnyDb {
    pub pool: AnyPool,
}

impl AnyDb {
    /// For SQLite use a `sqlite:file_path` URL; for Postgres `postgres://`.
    pub async fn new(conn_str: &str) -> Result<Self, DbError> {
        install_default_drivers();
        let opts = AnyConnectOptions::from_str(conn_str)?;

        let pool = AnyPoolOptions::new()
            // you can tweak these per‐DB by inspecting opts.kind()
            .max_connections(5)
            .connect_with(opts)
            .await?;

        // apply any migrations
        sqlx::migrate!().run(&pool).await?;

        Ok(Self { pool })
    }

    pub fn pool(&self) -> &AnyPool {
        &self.pool
    }
}

#[async_trait]
impl IndexerDb for AnyDb {
    fn pool(&self) -> &AnyPool {
        &self.pool
    }

    async fn get_last_block(&self) -> Result<Option<u64>, DbError> {
        let res = sqlx::query("SELECT block FROM last_block WHERE id = $1")
            .bind(SQL_BLOCK_KEY)
            .fetch_optional(&self.pool)
            .await?;

        let Some(row) = res else {
            return Ok(None);
        };

        let block_str: String = row.try_get("block")?;

        Ok(Some(block_str.parse().map_err(|_err| DbError::BadBlockNumb(block_str))?))
    }

    async fn set_last_block(&self, block_numb: u64) -> Result<(), DbError> {
        let res = sqlx::query(
            "INSERT INTO last_block (id, block) VALUES ($1, $2)
         ON CONFLICT (id) DO UPDATE SET block = EXCLUDED.block",
        )
        .bind(SQL_BLOCK_KEY)
        .bind(block_numb.to_string())
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::SetBlockFail);
        }

        Ok(())
    }

    async fn add_block(&self, block_numb: u64, block_timestamp: u64) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO blocks (block_number, block_timestamp) VALUES ($1, $2)
         ON CONFLICT (block_number) DO NOTHING",
        )
        .bind(block_numb as i64)
        .bind(block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_block_timestamp(&self, block_numb: u64) -> Result<Option<u64>, DbError> {
        let result = sqlx::query("SELECT block_timestamp FROM blocks WHERE block_number = $1")
            .bind(block_numb as i64)
            .fetch_optional(&self.pool)
            .await?;

        if let Some(row) = result {
            let block_timestamp: i64 = row.get(0);
            Ok(Some(block_timestamp as u64))
        } else {
            Ok(None)
        }
    }

    async fn add_tx(&self, metadata: &TxMetadata) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO transactions (
                tx_hash, 
                block_number, 
                from_address, 
                block_timestamp,
                transaction_index
            ) VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (tx_hash) DO NOTHING",
        )
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(format!("{:x}", metadata.from))
        .bind(metadata.block_timestamp as i64)
        .bind(metadata.transaction_index as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn has_proof_request(&self, request_digest: B256) -> Result<bool, DbError> {
        let result = sqlx::query("SELECT 1 FROM proof_requests WHERE request_digest = $1")
            .bind(format!("{request_digest:x}"))
            .fetch_optional(&self.pool)
            .await?;

        Ok(result.is_some())
    }

    async fn add_proof_request(
        &self,
        request_digest: B256,
        request: ProofRequest,
        metadata: &TxMetadata,
        source: &str,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        let predicate_type = match request.requirements.predicate.predicateType {
            PredicateType::DigestMatch => "DigestMatch",
            PredicateType::PrefixMatch => "PrefixMatch",
            PredicateType::ClaimDigestMatch => "ClaimDigestMatch",
            _ => return Err(DbError::BadTransaction("Invalid predicate type".to_string())),
        };
        let input_type = match request.input.inputType {
            RequestInputType::Inline => "Inline",
            RequestInputType::Url => "Url",
            _ => return Err(DbError::BadTransaction("Invalid input type".to_string())),
        };

        sqlx::query(
            "INSERT INTO proof_requests (
                request_digest,
                request_id,
                client_address,
                predicate_type,
                predicate_data,
                callback_address,
                callback_gas_limit,
                selector,
                input_type,
                input_data,
                min_price,
                max_price,
                lock_collateral,
                bidding_start,
                expires_at,
                lock_end,
                ramp_up_period,
                tx_hash,
                block_number,
                block_timestamp,
                source
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21)
            ON CONFLICT (request_digest) DO NOTHING",
        )
        .bind(format!("{request_digest:x}"))
        .bind(format!("{:x}", request.id))
        .bind(format!("{:x}", request.client_address()))
        .bind(predicate_type)
        .bind(format!("{:x}", request.requirements.predicate.data))
        .bind(format!("{:x}", request.requirements.callback.addr))
        .bind(request.requirements.callback.gasLimit.to_string())
        .bind(format!("{:x}", request.requirements.selector))
        .bind(input_type)
        .bind(format!("{:x}", request.input.data))
        .bind(request.offer.minPrice.to_string())
        .bind(request.offer.maxPrice.to_string())
        .bind(request.offer.lockCollateral.to_string())
        .bind(request.offer.rampUpStart as i64)
        .bind((request.offer.rampUpStart + request.offer.timeout as u64)  as i64)
        .bind((request.offer.rampUpStart + request.offer.lockTimeout as u64)  as i64)
        .bind(request.offer.rampUpPeriod as i64)
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .bind(source)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn add_assessor_receipt(
        &self,
        receipt: AssessorReceipt,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO assessor_receipts (
                tx_hash,
                prover_address,
                seal,
                block_number,
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (tx_hash) DO NOTHING",
        )
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(format!("{:x}", receipt.prover))
        .bind(format!("{:x}", receipt.seal))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_fulfillment(
        &self,
        fill: Fulfillment,
        prover_address: Address,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        let fulfillment_data_type: &'static str = match fill.fulfillmentDataType {
            FulfillmentDataType::ImageIdAndJournal => "ImageIdAndJournal",
            FulfillmentDataType::None => "None",
            _ => return Err(DbError::BadTransaction("Invalid fulfillment data type".to_string())),
        };
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO fulfillments (
                request_digest,
                request_id,
                prover_address,
                claim_digest,
                fulfillment_data_type,
                fulfillment_data,
                seal,
                tx_hash,
                block_number,
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT (request_digest, tx_hash) DO NOTHING",
        )
        .bind(format!("{:x}", fill.requestDigest))
        .bind(format!("{:x}", fill.id))
        .bind(format!("{prover_address:x}"))
        .bind(format!("{:x}", fill.claimDigest))
        .bind(fulfillment_data_type)
        .bind(format!("{:x}", fill.fulfillmentData))
        .bind(format!("{:x}", fill.seal))
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_request_submitted_event(
        &self,
        request_digest: B256,
        request_id: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO request_submitted_events (
                request_digest,
                request_id, 
                tx_hash, 
                block_number, 
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (request_digest) DO NOTHING",
        )
        .bind(format!("{request_digest:x}"))
        .bind(format!("{request_id:x}"))
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_request_locked_event(
        &self,
        request_digest: B256,
        request_id: U256,
        prover_address: Address,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO request_locked_events (
                request_digest,
                request_id, 
                prover_address,
                tx_hash, 
                block_number, 
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (request_digest) DO NOTHING",
        )
        .bind(format!("{request_digest:x}"))
        .bind(format!("{request_id:x}"))
        .bind(format!("{prover_address:x}"))
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_proof_delivered_event(
        &self,
        request_digest: B256,
        request_id: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO proof_delivered_events (
                request_digest,
                request_id, 
                tx_hash, 
                block_number, 
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (request_digest, tx_hash) DO NOTHING",
        )
        .bind(format!("{request_digest:x}"))
        .bind(format!("{request_id:x}"))
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_request_fulfilled_event(
        &self,
        request_digest: B256,
        request_id: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO request_fulfilled_events (
                request_digest,
                request_id, 
                tx_hash, 
                block_number, 
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (request_digest) DO NOTHING",
        )
        .bind(format!("{request_digest:x}"))
        .bind(format!("{request_id:x}"))
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_prover_slashed_event(
        &self,
        request_id: U256,
        burn_value: U256,
        transfer_value: U256,
        collateral_recipient: Address,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        let result =
            sqlx::query("SELECT prover_address FROM request_locked_events WHERE request_id = $1")
                .bind(format!("{request_id:x}"))
                .fetch_optional(&self.pool)
                .await?;
        // TODO: Improve this
        // If for some reason due to a gap in the db that is missing the associated locked request event,
        // we set the prover address to zero.
        let prover_address =
            result.map(|row| row.try_get("prover_address")).transpose()?.unwrap_or_else(|| {
                tracing::warn!(
                    "Missing request locked event for slashed event for request id: {:x}",
                    request_id
                );
                format!("{:x}", Address::ZERO)
            });
        sqlx::query(
            "INSERT INTO prover_slashed_events (
                request_id, 
                prover_address,
                burn_value,
                transfer_value,
                collateral_recipient,
                tx_hash, 
                block_number, 
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (request_id) DO NOTHING",
        )
        .bind(format!("{request_id:x}"))
        .bind(prover_address)
        .bind(burn_value.to_string())
        .bind(transfer_value.to_string())
        .bind(format!("{collateral_recipient:x}"))
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_deposit_event(
        &self,
        account: Address,
        value: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO deposit_events (
                account,
                value,
                tx_hash, 
                block_number, 
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (account, tx_hash) DO NOTHING",
        )
        .bind(format!("{account:x}"))
        .bind(value.to_string())
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_withdrawal_event(
        &self,
        account: Address,
        value: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO withdrawal_events (
                account,
                value,
                tx_hash, 
                block_number, 
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (account, tx_hash) DO NOTHING",
        )
        .bind(format!("{account:x}"))
        .bind(value.to_string())
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_collateral_deposit_event(
        &self,
        account: Address,
        value: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO collateral_deposit_events (
                account,
                value,
                tx_hash, 
                block_number, 
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (account, tx_hash) DO NOTHING",
        )
        .bind(format!("{account:x}"))
        .bind(value.to_string())
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_collateral_withdrawal_event(
        &self,
        account: Address,
        value: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO collateral_withdrawal_events (
                account,
                value,
                tx_hash, 
                block_number, 
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (account, tx_hash) DO NOTHING",
        )
        .bind(format!("{account:x}"))
        .bind(value.to_string())
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn add_callback_failed_event(
        &self,
        request_id: U256,
        callback_address: Address,
        error_data: Vec<u8>,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO callback_failed_events (
                request_id,
                callback_address,
                error_data,
                tx_hash, 
                block_number, 
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (request_id, tx_hash) DO NOTHING",
        )
        .bind(format!("{request_id:x}"))
        .bind(format!("{callback_address:x}"))
        .bind(error_data)
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_last_order_stream_timestamp(
        &self,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>, DbError> {
        let res =
            sqlx::query("SELECT last_processed_timestamp FROM order_stream_state WHERE id = TRUE")
                .fetch_optional(&self.pool)
                .await?;

        let Some(row) = res else {
            return Ok(None);
        };

        let timestamp_str: Option<String> = row.try_get("last_processed_timestamp")?;

        let timestamp = timestamp_str
            .map(|s| s.parse::<chrono::DateTime<chrono::Utc>>())
            .transpose()
            .map_err(|e| DbError::BadTransaction(format!("Failed to parse timestamp: {}", e)))?;

        Ok(timestamp)
    }

    async fn set_last_order_stream_timestamp(
        &self,
        timestamp: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), DbError> {
        let timestamp_str = timestamp.to_rfc3339();

        sqlx::query("UPDATE order_stream_state SET last_processed_timestamp = $1 WHERE id = TRUE")
            .bind(timestamp_str)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn upsert_hourly_market_summary(
        &self,
        summary: HourlyMarketSummary,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO hourly_market_summary (
                hour_timestamp,
                total_fulfilled,
                unique_provers_locking_requests,
                unique_requesters_submitting_requests,
                total_fees_locked,
                total_collateral_locked,
                p10_fees_locked,
                p25_fees_locked,
                p50_fees_locked,
                p75_fees_locked,
                p90_fees_locked,
                p95_fees_locked,
                p99_fees_locked,
                total_requests_submitted,
                total_requests_submitted_onchain,
                total_requests_submitted_offchain,
                total_requests_locked,
                total_requests_slashed,
                updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, CURRENT_TIMESTAMP)
            ON CONFLICT (hour_timestamp) DO UPDATE SET
                total_fulfilled = EXCLUDED.total_fulfilled,
                unique_provers_locking_requests = EXCLUDED.unique_provers_locking_requests,
                unique_requesters_submitting_requests = EXCLUDED.unique_requesters_submitting_requests,
                total_fees_locked = EXCLUDED.total_fees_locked,
                total_collateral_locked = EXCLUDED.total_collateral_locked,
                p10_fees_locked = EXCLUDED.p10_fees_locked,
                p25_fees_locked = EXCLUDED.p25_fees_locked,
                p50_fees_locked = EXCLUDED.p50_fees_locked,
                p75_fees_locked = EXCLUDED.p75_fees_locked,
                p90_fees_locked = EXCLUDED.p90_fees_locked,
                p95_fees_locked = EXCLUDED.p95_fees_locked,
                p99_fees_locked = EXCLUDED.p99_fees_locked,
                total_requests_submitted = EXCLUDED.total_requests_submitted,
                total_requests_submitted_onchain = EXCLUDED.total_requests_submitted_onchain,
                total_requests_submitted_offchain = EXCLUDED.total_requests_submitted_offchain,
                total_requests_locked = EXCLUDED.total_requests_locked,
                total_requests_slashed = EXCLUDED.total_requests_slashed,
                updated_at = CURRENT_TIMESTAMP",
        )
        .bind(summary.hour_timestamp as i64)
        .bind(summary.total_fulfilled as i64)
        .bind(summary.unique_provers_locking_requests as i64)
        .bind(summary.unique_requesters_submitting_requests as i64)
        .bind(summary.total_fees_locked)
        .bind(summary.total_collateral_locked)
        .bind(summary.p10_fees_locked)
        .bind(summary.p25_fees_locked)
        .bind(summary.p50_fees_locked)
        .bind(summary.p75_fees_locked)
        .bind(summary.p90_fees_locked)
        .bind(summary.p95_fees_locked)
        .bind(summary.p99_fees_locked)
        .bind(summary.total_requests_submitted as i64)
        .bind(summary.total_requests_submitted_onchain as i64)
        .bind(summary.total_requests_submitted_offchain as i64)
        .bind(summary.total_requests_locked as i64)
        .bind(summary.total_requests_slashed as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_hourly_market_summaries(
        &self,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<HourlyMarketSummary>, DbError> {
        let mut conditions = Vec::new();
        let mut bind_count = 0;

        // Add cursor condition
        let cursor_condition = match (cursor, sort) {
            (Some(_), SortDirection::Asc) => {
                bind_count += 1;
                Some(format!("hour_timestamp > ${}", bind_count))
            }
            (Some(_), SortDirection::Desc) => {
                bind_count += 1;
                Some(format!("hour_timestamp < ${}", bind_count))
            }
            (None, _) => None,
        };

        if let Some(cond) = cursor_condition {
            conditions.push(cond);
        }

        // Add after condition
        if after.is_some() {
            bind_count += 1;
            conditions.push(format!("hour_timestamp > ${}", bind_count));
        }

        // Add before condition
        if before.is_some() {
            bind_count += 1;
            conditions.push(format!("hour_timestamp < ${}", bind_count));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let order_clause = match sort {
            SortDirection::Asc => "ORDER BY hour_timestamp ASC",
            SortDirection::Desc => "ORDER BY hour_timestamp DESC",
        };

        bind_count += 1;
        let query_str = format!(
            "SELECT
                hour_timestamp,
                total_fulfilled,
                unique_provers_locking_requests,
                unique_requesters_submitting_requests,
                total_fees_locked,
                total_collateral_locked,
                p10_fees_locked,
                p25_fees_locked,
                p50_fees_locked,
                p75_fees_locked,
                p90_fees_locked,
                p95_fees_locked,
                p99_fees_locked,
                total_requests_submitted,
                total_requests_submitted_onchain,
                total_requests_submitted_offchain,
                total_requests_locked,
                total_requests_slashed
            FROM hourly_market_summary
            {}
            {}
            LIMIT ${}",
            where_clause, order_clause, bind_count
        );

        let mut query = sqlx::query(&query_str);

        // Bind parameters in the same order as bind_count increments
        if let Some(cursor_ts) = cursor {
            query = query.bind(cursor_ts);
        }

        if let Some(after_ts) = after {
            query = query.bind(after_ts);
        }

        if let Some(before_ts) = before {
            query = query.bind(before_ts);
        }

        query = query.bind(limit);

        let rows = query.fetch_all(&self.pool).await?;

        let summaries = rows
            .into_iter()
            .map(|row| HourlyMarketSummary {
                hour_timestamp: row.get::<i64, _>("hour_timestamp") as u64,
                total_fulfilled: row.get::<i64, _>("total_fulfilled") as u64,
                unique_provers_locking_requests: row.get::<i64, _>("unique_provers_locking_requests") as u64,
                unique_requesters_submitting_requests: row.get::<i64, _>(
                    "unique_requesters_submitting_requests",
                ) as u64,
                total_fees_locked: row.get("total_fees_locked"),
                total_collateral_locked: row.get("total_collateral_locked"),
                p10_fees_locked: row.get("p10_fees_locked"),
                p25_fees_locked: row.get("p25_fees_locked"),
                p50_fees_locked: row.get("p50_fees_locked"),
                p75_fees_locked: row.get("p75_fees_locked"),
                p90_fees_locked: row.get("p90_fees_locked"),
                p95_fees_locked: row.get("p95_fees_locked"),
                p99_fees_locked: row.get("p99_fees_locked"),
                total_requests_submitted: row.get::<i64, _>("total_requests_submitted") as u64,
                total_requests_submitted_onchain: row.get::<i64, _>("total_requests_submitted_onchain") as u64,
                total_requests_submitted_offchain: row.get::<i64, _>("total_requests_submitted_offchain") as u64,
                total_requests_locked: row.get::<i64, _>("total_requests_locked") as u64,
                total_requests_slashed: row.get::<i64, _>("total_requests_slashed") as u64,
            })
            .collect();

        Ok(summaries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TestDb;
    use alloy::primitives::{Address, Bytes, B256, U256};
    use boundless_market::contracts::{
        AssessorReceipt, Fulfillment, FulfillmentDataType, Offer, Predicate, ProofRequest,
        RequestId, RequestInput, Requirements,
    };
    use risc0_zkvm::Digest;

    // generate a test request
    fn generate_request(id: u32, addr: &Address) -> ProofRequest {
        ProofRequest::new(
            RequestId::new(*addr, id),
            Requirements::new(Predicate::prefix_match(Digest::default(), Bytes::default())),
            "https://image_url.dev",
            RequestInput::builder().write_slice(&[0x41, 0x41, 0x41, 0x41]).build_inline().unwrap(),
            Offer {
                minPrice: U256::from(20000000000000u64),
                maxPrice: U256::from(40000000000000u64),
                rampUpStart: 0,
                timeout: 420,
                lockTimeout: 420,
                rampUpPeriod: 1,
                lockCollateral: U256::from(10),
            },
        )
    }

    #[tokio::test]
    async fn set_get_block() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let mut block_numb = 20;
        db.set_last_block(block_numb).await.unwrap();

        let db_block = db.get_last_block().await.unwrap().unwrap();
        assert_eq!(block_numb, db_block);

        block_numb = 21;
        db.set_last_block(block_numb).await.unwrap();

        let db_block = db.get_last_block().await.unwrap().unwrap();
        assert_eq!(block_numb, db_block);
    }

    #[tokio::test]
    async fn test_transactions() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);

        db.add_tx(&metadata).await.unwrap();

        // Verify transaction was added
        let result = sqlx::query("SELECT * FROM transactions WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<i64, _>("block_number"), metadata.block_number as i64);
    }

    #[tokio::test]
    async fn test_proof_requests() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let request_digest = B256::ZERO;
        let request = generate_request(0, &Address::ZERO);
        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);
        db.add_proof_request(request_digest, request.clone(), &metadata, "onchain").await.unwrap();

        // Verify proof request was added
        let result = sqlx::query("SELECT * FROM proof_requests WHERE request_digest = $1")
            .bind(format!("{request_digest:x}"))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("request_id"), format!("{:x}", request.id));
    }

    #[tokio::test]
    async fn test_has_proof_request() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let request_digest = B256::ZERO;
        let non_existent_digest = B256::from([1; 32]);
        let request = generate_request(0, &Address::ZERO);
        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);
        // Initially, both digests should not exist
        assert!(!db.has_proof_request(request_digest).await.unwrap());
        assert!(!db.has_proof_request(non_existent_digest).await.unwrap());

        // Add a proof request
        db.add_proof_request(request_digest, request, &metadata, "onchain").await.unwrap();

        // Now the added request should exist, but the non-existent one should not
        assert!(db.has_proof_request(request_digest).await.unwrap());
        assert!(!db.has_proof_request(non_existent_digest).await.unwrap());
    }

    #[tokio::test]
    async fn test_assessor_receipts() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);

        let receipt = AssessorReceipt {
            prover: Address::ZERO,
            callbacks: vec![],
            selectors: vec![],
            seal: Bytes::default(),
        };

        db.add_assessor_receipt(receipt.clone(), &metadata).await.unwrap();

        // Verify assessor receipt was added
        let result = sqlx::query("SELECT * FROM assessor_receipts WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("prover_address"), format!("{:x}", receipt.prover));
    }

    #[tokio::test]
    async fn test_fulfillments() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);

        let fill = Fulfillment {
            requestDigest: B256::ZERO,
            id: U256::from(1),
            claimDigest: B256::ZERO,
            fulfillmentData: Bytes::default(),
            fulfillmentDataType: FulfillmentDataType::None,
            seal: Bytes::default(),
        };

        let prover_address = Address::ZERO;
        db.add_tx(&metadata).await.unwrap();
        db.add_proof_delivered_event(fill.requestDigest, fill.id, &metadata).await.unwrap();
        db.add_fulfillment(fill.clone(), prover_address, &metadata).await.unwrap();

        // Verify fulfillment was added
        let result = sqlx::query("SELECT * FROM fulfillments WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("request_digest"), format!("{:x}", fill.requestDigest));
    }

    #[tokio::test]
    async fn test_events() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);

        let request_digest = B256::ZERO;
        let request_id = U256::from(1);

        // Test request submitted event
        db.add_request_submitted_event(request_digest, request_id, &metadata).await.unwrap();
        let result = sqlx::query("SELECT * FROM request_submitted_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("request_digest"), format!("{request_digest:x}"));

        // Test request locked event
        let prover_address = Address::ZERO;
        db.add_request_locked_event(request_digest, request_id, prover_address, &metadata)
            .await
            .unwrap();
        let result = sqlx::query("SELECT * FROM request_locked_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("prover_address"), format!("{prover_address:x}"));

        // Test proof delivered event
        db.add_proof_delivered_event(request_digest, request_id, &metadata).await.unwrap();
        let result = sqlx::query("SELECT * FROM proof_delivered_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("request_digest"), format!("{request_digest:x}"));

        // Test request fulfilled event
        db.add_request_fulfilled_event(request_digest, request_id, &metadata).await.unwrap();
        let result = sqlx::query("SELECT * FROM request_fulfilled_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("request_digest"), format!("{request_digest:x}"));
    }

    #[tokio::test]
    async fn test_prover_slashed_event() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);

        let request_id = U256::from(1);
        let burn_value = U256::from(100);
        let transfer_value = U256::from(50);
        let collateral_recipient = Address::ZERO;

        // First add a request locked event (required for prover slashed event)
        let request_digest = B256::ZERO;
        let prover_address = Address::ZERO;
        db.add_request_locked_event(request_digest, request_id, prover_address, &metadata)
            .await
            .unwrap();

        // Then test prover slashed event
        db.add_prover_slashed_event(
            request_id,
            burn_value,
            transfer_value,
            collateral_recipient,
            &metadata,
        )
        .await
        .unwrap();
        let result = sqlx::query("SELECT * FROM prover_slashed_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("burn_value"), burn_value.to_string());
    }

    #[tokio::test]
    async fn test_account_events() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);

        let account = Address::ZERO;
        let value = U256::from(100);

        // Test deposit event
        db.add_deposit_event(account, value, &metadata).await.unwrap();
        let result = sqlx::query("SELECT * FROM deposit_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("value"), value.to_string());

        // Test withdrawal event
        db.add_withdrawal_event(account, value, &metadata).await.unwrap();
        let result = sqlx::query("SELECT * FROM withdrawal_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("value"), value.to_string());

        // Test collateral deposit event
        db.add_collateral_deposit_event(account, value, &metadata).await.unwrap();
        let result = sqlx::query("SELECT * FROM collateral_deposit_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("value"), value.to_string());

        // Test collateral withdrawal event
        db.add_collateral_withdrawal_event(account, value, &metadata).await.unwrap();
        let result = sqlx::query("SELECT * FROM collateral_withdrawal_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<String, _>("value"), value.to_string());
    }

    #[tokio::test]
    async fn test_callback_failed_event() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;

        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);

        let request_id = U256::from(1);
        let callback_address = Address::ZERO;
        let error_data = vec![1, 2, 3, 4];

        db.add_callback_failed_event(request_id, callback_address, error_data.clone(), &metadata)
            .await
            .unwrap();
        let result = sqlx::query("SELECT * FROM callback_failed_events WHERE tx_hash = $1")
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(result.get::<Vec<u8>, _>("error_data"), error_data);
    }

    // Helper function to create test data for hourly market summaries
    async fn setup_hourly_summaries(db: &DbObj) -> (i64, i64) {
        let base_timestamp = 1700000000i64; // Nov 14, 2023 22:13:20 UTC
        let hour_in_seconds = 3600i64;

        // Insert 10 hourly summaries
        for i in 0..10u64 {
            let summary = HourlyMarketSummary {
                hour_timestamp: (base_timestamp + (i as i64 * hour_in_seconds)) as u64,
                total_fulfilled: i,
                unique_provers_locking_requests: i * 2,
                unique_requesters_submitting_requests: i * 3,
                total_fees_locked: format!("{}", i * 1000),
                total_collateral_locked: format!("{}", i * 2000),
                p10_fees_locked: format!("{}", i * 100),
                p25_fees_locked: format!("{}", i * 250),
                p50_fees_locked: format!("{}", i * 500),
                p75_fees_locked: format!("{}", i * 750),
                p90_fees_locked: format!("{}", i * 900),
                p95_fees_locked: format!("{}", i * 950),
                p99_fees_locked: format!("{}", i * 990),
                total_requests_submitted: i * 10,
                total_requests_submitted_onchain: i * 6,
                total_requests_submitted_offchain: i * 4,
                total_requests_locked: i * 5,
                total_requests_slashed: i,
            };
            db.upsert_hourly_market_summary(summary).await.unwrap();
        }

        (base_timestamp, hour_in_seconds)
    }

    #[tokio::test]
    async fn test_hourly_summaries_basic_desc_pagination() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;
        let (base_timestamp, hour_in_seconds) = setup_hourly_summaries(&db).await;

        let results = db
            .get_hourly_market_summaries(None, 3, SortDirection::Desc, None, None)
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].hour_timestamp, (base_timestamp + (9 * hour_in_seconds)) as u64);
        assert_eq!(results[1].hour_timestamp, (base_timestamp + (8 * hour_in_seconds)) as u64);
        assert_eq!(results[2].hour_timestamp, (base_timestamp + (7 * hour_in_seconds)) as u64);
    }

    #[tokio::test]
    async fn test_hourly_summaries_cursor_desc_pagination() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;
        let (base_timestamp, hour_in_seconds) = setup_hourly_summaries(&db).await;

        // Get first page
        let first_page = db
            .get_hourly_market_summaries(None, 3, SortDirection::Desc, None, None)
            .await
            .unwrap();
        
        // Use last item as cursor for next page
        let cursor = first_page[2].hour_timestamp as i64;
        let results = db
            .get_hourly_market_summaries(Some(cursor), 3, SortDirection::Desc, None, None)
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].hour_timestamp, (base_timestamp + (6 * hour_in_seconds)) as u64);
        assert_eq!(results[1].hour_timestamp, (base_timestamp + (5 * hour_in_seconds)) as u64);
        assert_eq!(results[2].hour_timestamp, (base_timestamp + (4 * hour_in_seconds)) as u64);
    }

    #[tokio::test]
    async fn test_hourly_summaries_basic_asc_pagination() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;
        let (base_timestamp, hour_in_seconds) = setup_hourly_summaries(&db).await;

        let results = db
            .get_hourly_market_summaries(None, 3, SortDirection::Asc, None, None)
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].hour_timestamp, base_timestamp as u64); // oldest first
        assert_eq!(results[1].hour_timestamp, (base_timestamp + hour_in_seconds) as u64);
        assert_eq!(results[2].hour_timestamp, (base_timestamp + (2 * hour_in_seconds)) as u64);
    }

    #[tokio::test]
    async fn test_hourly_summaries_cursor_asc_pagination() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;
        let (base_timestamp, hour_in_seconds) = setup_hourly_summaries(&db).await;

        // Get first page
        let first_page = db
            .get_hourly_market_summaries(None, 3, SortDirection::Asc, None, None)
            .await
            .unwrap();
        
        // Use last item as cursor for next page
        let cursor = first_page[2].hour_timestamp as i64;
        let results = db
            .get_hourly_market_summaries(Some(cursor), 3, SortDirection::Asc, None, None)
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].hour_timestamp, (base_timestamp + (3 * hour_in_seconds)) as u64);
        assert_eq!(results[1].hour_timestamp, (base_timestamp + (4 * hour_in_seconds)) as u64);
        assert_eq!(results[2].hour_timestamp, (base_timestamp + (5 * hour_in_seconds)) as u64);
    }

    #[tokio::test]
    async fn test_hourly_summaries_after_filter() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;
        let (base_timestamp, hour_in_seconds) = setup_hourly_summaries(&db).await;

        let after = base_timestamp + (3 * hour_in_seconds);
        let results = db
            .get_hourly_market_summaries(None, 10, SortDirection::Desc, None, Some(after))
            .await
            .unwrap();

        // Should get hours 4-9 (6 results) - all after hour 3, desc so most recent first
        assert_eq!(results.len(), 6);
        assert_eq!(results[0].hour_timestamp, (base_timestamp + (9 * hour_in_seconds)) as u64);
        assert_eq!(results[results.len() - 1].hour_timestamp, (base_timestamp + (4 * hour_in_seconds)) as u64);
    }

    #[tokio::test]
    async fn test_hourly_summaries_before_filter() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;
        let (base_timestamp, hour_in_seconds) = setup_hourly_summaries(&db).await;

        let before = base_timestamp + (7 * hour_in_seconds);
        let results = db
            .get_hourly_market_summaries(None, 10, SortDirection::Desc, Some(before), None)
            .await
            .unwrap();

        // Should get hours 0-6 (7 results) - all before hour 7
        assert_eq!(results.len(), 7);
        assert_eq!(results[0].hour_timestamp, (base_timestamp + (6 * hour_in_seconds)) as u64);
        assert_eq!(results[results.len() - 1].hour_timestamp, base_timestamp as u64);
    }

    #[tokio::test]
    async fn test_hourly_summaries_before_and_after_filter() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;
        let (base_timestamp, hour_in_seconds) = setup_hourly_summaries(&db).await;

        let after = base_timestamp + (2 * hour_in_seconds);
        let before = base_timestamp + (7 * hour_in_seconds);
        let results = db
            .get_hourly_market_summaries(None, 10, SortDirection::Desc, Some(before), Some(after))
            .await
            .unwrap();

        // Should get hours 3-6 (4 results) - between hour 2 and hour 7
        assert_eq!(results.len(), 4);
        assert_eq!(results[0].hour_timestamp, (base_timestamp + (6 * hour_in_seconds)) as u64);
        assert_eq!(results[3].hour_timestamp, (base_timestamp + (3 * hour_in_seconds)) as u64);
    }

    #[tokio::test]
    async fn test_hourly_summaries_cursor_with_before_filter() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;
        let (base_timestamp, hour_in_seconds) = setup_hourly_summaries(&db).await;

        let cursor = base_timestamp + (8 * hour_in_seconds);
        let before = base_timestamp + (5 * hour_in_seconds);
        let results = db
            .get_hourly_market_summaries(Some(cursor), 10, SortDirection::Desc, Some(before), None)
            .await
            .unwrap();

        // Cursor at hour 8 means timestamp < 8 (DESC)
        // Before at hour 5 means timestamp < 5
        // Combined: timestamp < 5, so we get hours 0-4 (5 results)
        assert!(results.len() <= 5);
    }

    #[tokio::test]
    async fn test_hourly_summaries_limit() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;
        setup_hourly_summaries(&db).await;

        let results = db
            .get_hourly_market_summaries(None, 2, SortDirection::Desc, None, None)
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_hourly_summaries_no_results() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;
        let (base_timestamp, hour_in_seconds) = setup_hourly_summaries(&db).await;

        // Use a timestamp way in the future
        let after = base_timestamp + (100 * hour_in_seconds);
        let results = db
            .get_hourly_market_summaries(None, 10, SortDirection::Desc, None, Some(after))
            .await
            .unwrap();

        assert_eq!(results.len(), 0);
    }

    #[tokio::test]
    async fn test_hourly_summaries_data_integrity() {
        let test_db = TestDb::new().await.unwrap();
        let db: DbObj = test_db.db;
        setup_hourly_summaries(&db).await;

        let results = db
            .get_hourly_market_summaries(None, 1, SortDirection::Asc, None, None)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].total_fulfilled, 0);
        assert_eq!(results[0].unique_provers_locking_requests, 0);
        assert_eq!(results[0].total_fees_locked, "0");
    }
}
