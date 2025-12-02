//! Extension trait for prover-related database operations.

use super::market::{IndexerDb, RequestCursor, RequestSortField, RequestStatus};
use super::DbError;
use alloy::primitives::Address;
use async_trait::async_trait;

/// Trait for prover-related database operations.
/// Requires IndexerDb for pool() and row_to_request_status() access.
#[async_trait]
pub trait ProversDb: IndexerDb {
    /// List requests where the prover either locked or fulfilled
    async fn list_requests_by_prover(
        &self,
        prover_address: Address,
        cursor: Option<RequestCursor>,
        limit: u32,
        sort_by: RequestSortField,
    ) -> Result<(Vec<RequestStatus>, Option<RequestCursor>), DbError> {
        let prover_str = format!("{:x}", prover_address);
        let sort_field = match sort_by {
            RequestSortField::UpdatedAt => "updated_at",
            RequestSortField::CreatedAt => "created_at",
        };

        let rows = if let Some(c) = &cursor {
            let query_str = format!(
                "SELECT * FROM request_status
                 WHERE (lock_prover_address = $1 OR fulfill_prover_address = $1)
                   AND ({} < $2 OR ({} = $2 AND request_digest < $3))
                 ORDER BY {} DESC, request_digest DESC
                 LIMIT $4",
                sort_field, sort_field, sort_field
            );
            sqlx::query(&query_str)
                .bind(&prover_str)
                .bind(c.timestamp as i64)
                .bind(&c.request_digest)
                .bind(limit as i64)
                .fetch_all(self.pool())
                .await?
        } else {
            let query_str = format!(
                "SELECT * FROM request_status
                 WHERE (lock_prover_address = $1 OR fulfill_prover_address = $1)
                 ORDER BY {} DESC, request_digest DESC
                 LIMIT $2",
                sort_field
            );
            sqlx::query(&query_str)
                .bind(&prover_str)
                .bind(limit as i64)
                .fetch_all(self.pool())
                .await?
        };

        let mut results = Vec::new();
        for row in rows {
            results.push(self.row_to_request_status(&row)?);
        }

        let next_cursor = if results.len() == limit as usize {
            results.last().map(|r| {
                let timestamp = match sort_by {
                    RequestSortField::UpdatedAt => r.updated_at,
                    RequestSortField::CreatedAt => r.created_at,
                };
                RequestCursor { timestamp, request_digest: r.request_digest.to_string() }
            })
        } else {
            None
        };

        Ok((results, next_cursor))
    }
}

// Blanket implementation for anything that implements IndexerDb
impl<T: IndexerDb + Send + Sync> ProversDb for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::market::{RequestStatusType, SlashedStatus};
    use crate::test_utils::TestDb;
    use alloy::primitives::{B256, U256};

    #[tokio::test]
    async fn test_list_requests_by_prover() {
        let test_db = TestDb::new().await.unwrap();
        let db = &test_db.db;

        let prover1 = Address::from([0xAA; 20]);
        let prover2 = Address::from([0xBB; 20]);
        let prover3 = Address::from([0xCC; 20]);
        let client_addr = Address::from([0x11; 20]);
        let base_ts = 1700000000u64;

        // Create requests where prover1 only locked (not fulfilled)
        for i in 0..3 {
            let status = RequestStatus {
                request_digest: B256::from([i as u8; 32]),
                request_id: U256::from(i),
                request_status: RequestStatusType::Locked,
                slashed_status: SlashedStatus::NotApplicable,
                source: "onchain".to_string(),
                client_address: client_addr,
                lock_prover_address: Some(prover1),
                fulfill_prover_address: None,
                created_at: base_ts + (i as u64 * 100),
                updated_at: base_ts + (i as u64 * 100),
                locked_at: Some(base_ts + (i as u64 * 100)),
                fulfilled_at: None,
                slashed_at: None,
                lock_prover_delivered_proof_at: None,
                submit_block: Some(100),
                lock_block: Some(101),
                fulfill_block: None,
                slashed_block: None,
                min_price: "1000".to_string(),
                max_price: "2000".to_string(),
                lock_collateral: "1000".to_string(),
                ramp_up_start: base_ts,
                ramp_up_period: 10,
                expires_at: base_ts + 10000,
                lock_end: base_ts + 10000,
                slash_recipient: None,
                slash_transferred_amount: None,
                slash_burned_amount: None,
                program_cycles: None,
                total_cycles: None,
                peak_prove_mhz: None,
                effective_prove_mhz: None,
                cycle_status: None,
                lock_price: Some("1500".to_string()),
                lock_price_per_cycle: Some("100".to_string()),
                submit_tx_hash: Some(B256::ZERO),
                lock_tx_hash: Some(B256::from([0x01; 32])),
                fulfill_tx_hash: None,
                slash_tx_hash: None,
                image_id: "test".to_string(),
                image_url: None,
                selector: "test".to_string(),
                predicate_type: "digest_match".to_string(),
                predicate_data: "0x00".to_string(),
                input_type: "inline".to_string(),
                input_data: "0x00".to_string(),
                fulfill_journal: None,
                fulfill_seal: None,
            };
            db.upsert_request_statuses(&[status]).await.unwrap();
        }

        // Create requests where prover1 both locked and fulfilled
        for i in 3..5 {
            let status = RequestStatus {
                request_digest: B256::from([i as u8; 32]),
                request_id: U256::from(i),
                request_status: RequestStatusType::Fulfilled,
                slashed_status: SlashedStatus::NotApplicable,
                source: "onchain".to_string(),
                client_address: client_addr,
                lock_prover_address: Some(prover1),
                fulfill_prover_address: Some(prover1),
                created_at: base_ts + (i as u64 * 100),
                updated_at: base_ts + (i as u64 * 100) + 50,
                locked_at: Some(base_ts + (i as u64 * 100)),
                fulfilled_at: Some(base_ts + (i as u64 * 100) + 50),
                slashed_at: None,
                lock_prover_delivered_proof_at: None,
                submit_block: Some(100),
                lock_block: Some(101),
                fulfill_block: Some(102),
                slashed_block: None,
                min_price: "1000".to_string(),
                max_price: "2000".to_string(),
                lock_collateral: "1000".to_string(),
                ramp_up_start: base_ts,
                ramp_up_period: 10,
                expires_at: base_ts + 10000,
                lock_end: base_ts + 10000,
                slash_recipient: None,
                slash_transferred_amount: None,
                slash_burned_amount: None,
                program_cycles: Some(U256::from(1000000)),
                total_cycles: Some(U256::from(1015800)),
                peak_prove_mhz: Some(1000),
                effective_prove_mhz: Some(950),
                cycle_status: Some("COMPLETED".to_string()),
                lock_price: Some("1500".to_string()),
                lock_price_per_cycle: Some("100".to_string()),
                submit_tx_hash: Some(B256::ZERO),
                lock_tx_hash: Some(B256::from([0x01; 32])),
                fulfill_tx_hash: Some(B256::from([0x02; 32])),
                slash_tx_hash: None,
                image_id: "test".to_string(),
                image_url: None,
                selector: "test".to_string(),
                predicate_type: "digest_match".to_string(),
                predicate_data: "0x00".to_string(),
                input_type: "inline".to_string(),
                input_data: "0x00".to_string(),
                fulfill_journal: Some("0x1234".to_string()),
                fulfill_seal: Some("0x5678".to_string()),
            };
            db.upsert_request_statuses(&[status]).await.unwrap();
        }

        // Create request where prover2 locked but prover1 fulfilled
        let status_mixed = RequestStatus {
            request_digest: B256::from([0x10; 32]),
            request_id: U256::from(10),
            request_status: RequestStatusType::Fulfilled,
            slashed_status: SlashedStatus::NotApplicable,
            source: "onchain".to_string(),
            client_address: client_addr,
            lock_prover_address: Some(prover2),
            fulfill_prover_address: Some(prover1),
            created_at: base_ts + 1000,
            updated_at: base_ts + 1050,
            locked_at: Some(base_ts + 1000),
            fulfilled_at: Some(base_ts + 1050),
            slashed_at: None,
            lock_prover_delivered_proof_at: None,
            submit_block: Some(100),
            lock_block: Some(101),
            fulfill_block: Some(102),
            slashed_block: None,
            min_price: "1000".to_string(),
            max_price: "2000".to_string(),
            lock_collateral: "1000".to_string(),
            ramp_up_start: base_ts,
            ramp_up_period: 10,
            expires_at: base_ts + 10000,
            lock_end: base_ts + 10000,
            slash_recipient: None,
            slash_transferred_amount: None,
            slash_burned_amount: None,
            program_cycles: Some(U256::from(2000000)),
            total_cycles: Some(U256::from(2031600)),
            peak_prove_mhz: Some(2000),
            effective_prove_mhz: Some(1900),
            cycle_status: Some("COMPLETED".to_string()),
            lock_price: Some("1500".to_string()),
            lock_price_per_cycle: Some("100".to_string()),
            submit_tx_hash: Some(B256::ZERO),
            lock_tx_hash: Some(B256::from([0x03; 32])),
            fulfill_tx_hash: Some(B256::from([0x04; 32])),
            slash_tx_hash: None,
            image_id: "test".to_string(),
            image_url: None,
            selector: "test".to_string(),
            predicate_type: "digest_match".to_string(),
            predicate_data: "0x00".to_string(),
            input_type: "inline".to_string(),
            input_data: "0x00".to_string(),
            fulfill_journal: Some("0xabcd".to_string()),
            fulfill_seal: Some("0xef01".to_string()),
        };
        db.upsert_request_statuses(&[status_mixed]).await.unwrap();

        // Create request where prover3 only fulfilled (didn't lock)
        let status_fulfill_only = RequestStatus {
            request_digest: B256::from([0x20; 32]),
            request_id: U256::from(20),
            request_status: RequestStatusType::Fulfilled,
            slashed_status: SlashedStatus::NotApplicable,
            source: "onchain".to_string(),
            client_address: client_addr,
            lock_prover_address: None,
            fulfill_prover_address: Some(prover3),
            created_at: base_ts + 2000,
            updated_at: base_ts + 2050,
            locked_at: None,
            fulfilled_at: Some(base_ts + 2050),
            slashed_at: None,
            lock_prover_delivered_proof_at: None,
            submit_block: Some(100),
            lock_block: None,
            fulfill_block: Some(102),
            slashed_block: None,
            min_price: "1000".to_string(),
            max_price: "2000".to_string(),
            lock_collateral: "0".to_string(),
            ramp_up_start: base_ts,
            ramp_up_period: 10,
            expires_at: base_ts + 10000,
            lock_end: base_ts + 10000,
            slash_recipient: None,
            slash_transferred_amount: None,
            slash_burned_amount: None,
            program_cycles: Some(U256::from(3000000)),
            total_cycles: Some(U256::from(3047400)),
            peak_prove_mhz: Some(3000),
            effective_prove_mhz: Some(2800),
            cycle_status: Some("COMPLETED".to_string()),
            lock_price: None,
            lock_price_per_cycle: None,
            submit_tx_hash: Some(B256::ZERO),
            lock_tx_hash: None,
            fulfill_tx_hash: Some(B256::from([0x05; 32])),
            slash_tx_hash: None,
            image_id: "test".to_string(),
            image_url: None,
            selector: "test".to_string(),
            predicate_type: "digest_match".to_string(),
            predicate_data: "0x00".to_string(),
            input_type: "inline".to_string(),
            input_data: "0x00".to_string(),
            fulfill_journal: Some("0xfedc".to_string()),
            fulfill_seal: Some("0xba98".to_string()),
        };
        db.upsert_request_statuses(&[status_fulfill_only]).await.unwrap();

        // List requests for prover1 - should get all requests where prover1 locked OR fulfilled
        // This includes: 3 locked-only, 2 locked+fulfilled, 1 fulfilled-only (mixed)
        // Total: 6 requests
        let (results, _cursor) = db
            .list_requests_by_prover(prover1, None, 10, RequestSortField::CreatedAt)
            .await
            .unwrap();
        assert_eq!(
            results.len(),
            6,
            "prover1 should have 6 requests (3 locked-only + 2 locked+fulfilled + 1 fulfilled-only)"
        );
        assert!(
            results.iter().all(|r| {
                r.lock_prover_address == Some(prover1) || r.fulfill_prover_address == Some(prover1)
            }),
            "All results should have prover1 as lock_prover or fulfill_prover"
        );

        // List with limit
        let (results, cursor) = db
            .list_requests_by_prover(prover1, None, 2, RequestSortField::CreatedAt)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert!(cursor.is_some());

        // Use cursor for pagination
        let (results2, _) = db
            .list_requests_by_prover(prover1, cursor, 2, RequestSortField::CreatedAt)
            .await
            .unwrap();
        assert_eq!(results2.len(), 2);
        // Results should be different from first page
        assert_ne!(results[0].request_id, results2[0].request_id);

        // Test sorting by updated_at
        let (results_updated, _) = db
            .list_requests_by_prover(prover1, None, 10, RequestSortField::UpdatedAt)
            .await
            .unwrap();
        assert_eq!(results_updated.len(), 6);
        // Verify descending order
        if results_updated.len() >= 2 {
            assert!(
                results_updated[0].updated_at >= results_updated[1].updated_at,
                "Should be sorted by updated_at descending"
            );
        }

        // List for prover2 - should only get the one request where prover2 locked
        let (results, _) = db
            .list_requests_by_prover(prover2, None, 10, RequestSortField::CreatedAt)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].lock_prover_address, Some(prover2));
        assert_eq!(results[0].fulfill_prover_address, Some(prover1)); // prover1 fulfilled it

        // List for prover3 - should only get the one request where prover3 fulfilled
        let (results, _) = db
            .list_requests_by_prover(prover3, None, 10, RequestSortField::CreatedAt)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].lock_prover_address, None);
        assert_eq!(results[0].fulfill_prover_address, Some(prover3));

        // List for a prover with no requests - should return empty
        let prover_no_requests = Address::from([0xFF; 20]);
        let (results, _) = db
            .list_requests_by_prover(prover_no_requests, None, 10, RequestSortField::CreatedAt)
            .await
            .unwrap();
        assert_eq!(results.len(), 0);
    }
}
