// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{str::FromStr, sync::Arc};

use alloy::primitives::{Address, B256, U256};
use async_trait::async_trait;
use boundless_market::contracts::{
    AssessorReceipt, Fulfillment, InputType, PredicateType, ProofRequest,
};
use sqlx::{
    any::{install_default_drivers, AnyConnectOptions, AnyPoolOptions},
    AnyPool, Row,
};
use thiserror::Error;

const SQL_BLOCK_KEY: i64 = 0;

#[derive(Debug, Clone)]
pub struct TxMetadata {
    pub tx_hash: B256,
    pub from: Address,
    pub block_number: u64,
    pub block_timestamp: u64,
}

impl TxMetadata {
    pub fn new(tx_hash: B256, from: Address, block_number: u64, block_timestamp: u64) -> Self {
        Self { tx_hash, from, block_number, block_timestamp }
    }
}

#[derive(Error, Debug)]
pub enum DbError {
    #[error("SQL error")]
    SqlErr(#[from] sqlx::Error),

    #[error("SQL Migration error")]
    MigrateErr(#[from] sqlx::migrate::MigrateError),

    #[error("Invalid block number: {0}")]
    BadBlockNumb(String),

    #[error("Failed to set last block")]
    SetBlockFail,

    #[error("Invalid transaction: {0}")]
    BadTransaction(String),
}

#[async_trait]
pub trait IndexerDb {
    async fn get_last_block(&self) -> Result<Option<u64>, DbError>;
    async fn set_last_block(&self, block_numb: u64) -> Result<(), DbError>;

    async fn add_tx(&self, metadata: &TxMetadata) -> Result<(), DbError>;

    async fn add_proof_request(
        &self,
        request_digest: B256,
        request: ProofRequest,
    ) -> Result<(), DbError>;

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
        stake_recipient: Address,
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

    async fn add_stake_deposit_event(
        &self,
        account: Address,
        value: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError>;

    async fn add_stake_withdrawal_event(
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
}

pub type DbObj = Arc<dyn IndexerDb + Send + Sync>;

#[derive(Debug, Clone)]
pub struct AnyDb {
    pool: AnyPool,
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
}

#[async_trait]
impl IndexerDb for AnyDb {
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

    async fn add_tx(&self, metadata: &TxMetadata) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO transactions (
                tx_hash, 
                block_number, 
                from_address, 
                block_timestamp
            ) VALUES ($1, $2, $3, $4)
            ON CONFLICT (tx_hash) DO NOTHING",
        )
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(format!("{:x}", metadata.from))
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn add_proof_request(
        &self,
        request_digest: B256,
        request: ProofRequest,
    ) -> Result<(), DbError> {
        let predicate_type = match request.requirements.predicate.predicateType {
            PredicateType::DigestMatch => "DigestMatch",
            PredicateType::PrefixMatch => "PrefixMatch",
            _ => return Err(DbError::BadTransaction("Invalid predicate type".to_string())),
        };
        let input_type = match request.input.inputType {
            InputType::Inline => "Inline",
            InputType::Url => "Url",
            _ => return Err(DbError::BadTransaction("Invalid input type".to_string())),
        };

        sqlx::query(
            "INSERT INTO proof_requests (
                request_digest,
                request_id, 
                client_address,
                image_id,
                predicate_type,
                predicate_data,
                callback_address,
                callback_gas_limit,
                selector,
                input_type,
                input_data,
                min_price,
                max_price,
                lock_stake,
                bidding_start,
                expires_at,
                lock_end,
                ramp_up_period
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)
            ON CONFLICT (request_digest) DO NOTHING",
        )
        .bind(format!("{:x}", request_digest))
        .bind(format!("{:x}", request.id))
        .bind(format!("{:x}", request.client_address()))
        .bind(format!("{:x}", request.requirements.imageId))
        .bind(predicate_type)
        .bind(format!("{:x}", request.requirements.predicate.data))
        .bind(format!("{:x}", request.requirements.callback.addr))
        .bind(request.requirements.callback.gasLimit.to_string())
        .bind(format!("{:x}", request.requirements.selector))
        .bind(input_type)
        .bind(format!("{:x}", request.input.data))
        .bind(request.offer.minPrice.to_string())
        .bind(request.offer.maxPrice.to_string())
        .bind(request.offer.lockStake.to_string())
        .bind(request.offer.biddingStart as i64)
        .bind((request.offer.biddingStart + request.offer.timeout as u64)  as i64)
        .bind((request.offer.biddingStart + request.offer.lockTimeout as u64)  as i64)
        .bind(request.offer.rampUpPeriod as i64)
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
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO fulfillments (
                request_digest,
                request_id,
                prover_address,
                image_id,
                journal,
                seal,
                tx_hash,
                block_number,
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (request_digest, tx_hash) DO NOTHING",
        )
        .bind(format!("{:x}", fill.requestDigest))
        .bind(format!("{:x}", fill.id))
        .bind(format!("{:x}", prover_address))
        .bind(format!("{:x}", fill.imageId))
        .bind(format!("{:x}", fill.journal))
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
        .bind(format!("{:x}", request_digest))
        .bind(format!("{:x}", request_id))
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
        .bind(format!("{:x}", request_digest))
        .bind(format!("{:x}", request_id))
        .bind(format!("{:x}", prover_address))
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
             ON CONFLICT (request_digest) DO NOTHING",
        )
        .bind(format!("{:x}", request_digest))
        .bind(format!("{:x}", request_id))
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
        .bind(format!("{:x}", request_digest))
        .bind(format!("{:x}", request_id))
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
        stake_recipient: Address,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        let resutl =
            sqlx::query("SELECT prover_address FROM request_locked_events WHERE request_id = $1")
                .bind(format!("{:x}", request_id))
                .fetch_one(&self.pool)
                .await?;
        let prover_address: String = resutl.try_get("prover_address")?;
        sqlx::query(
            "INSERT INTO prover_slashed_events (
                request_id, 
                prover_address,
                burn_value,
                transfer_value,
                stake_recipient,
                tx_hash, 
                block_number, 
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (request_id) DO NOTHING",
        )
        .bind(format!("{:x}", request_id))
        .bind(prover_address)
        .bind(burn_value.to_string())
        .bind(transfer_value.to_string())
        .bind(format!("{:x}", stake_recipient))
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
        .bind(format!("{:x}", account))
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
        .bind(format!("{:x}", account))
        .bind(value.to_string())
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    async fn add_stake_deposit_event(
        &self,
        account: Address,
        value: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO stake_deposit_events (
                account,
                value,
                tx_hash, 
                block_number, 
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (account, tx_hash) DO NOTHING",
        )
        .bind(format!("{:x}", account))
        .bind(value.to_string())
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
    async fn add_stake_withdrawal_event(
        &self,
        account: Address,
        value: U256,
        metadata: &TxMetadata,
    ) -> Result<(), DbError> {
        self.add_tx(metadata).await?;
        sqlx::query(
            "INSERT INTO stake_withdrawal_events (
                account,
                value,
                tx_hash, 
                block_number, 
                block_timestamp
            ) VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (account, tx_hash) DO NOTHING",
        )
        .bind(format!("{:x}", account))
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
             ON CONFLICT (request_id) DO NOTHING",
        )
        .bind(format!("{:x}", request_id))
        .bind(format!("{:x}", callback_address))
        .bind(error_data)
        .bind(format!("{:x}", metadata.tx_hash))
        .bind(metadata.block_number as i64)
        .bind(metadata.block_timestamp as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TestDb;

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
}
