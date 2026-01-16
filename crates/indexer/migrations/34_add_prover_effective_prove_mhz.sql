-- Add prover_effective_prove_mhz column to request_status
-- This field measures prover performance from their perspective:
-- - Primary fulfillment (lock holder): total_cycles / (fulfilled_at - locked_at)
-- - Secondary fulfillment (lock expired + different prover): total_cycles / (fulfilled_at - lock_end)
-- - No lock: total_cycles / (fulfilled_at - created_at)

ALTER TABLE request_status ADD COLUMN prover_effective_prove_mhz DOUBLE PRECISION;
