CREATE INDEX IF NOT EXISTS idx_prover_slashed_events_collateral_recipient
    ON prover_slashed_events (collateral_recipient);
