use alloy_sol_types::sol;
use risc0_zkvm::{digest, sha::Digest};

// TODO: Provide a real value here, and extract to a file than can be auto-generated.
const _LOG_BUILDER_ID: Digest =
    digest!("0000000000000000000000000000000000000000000000000000000000000000");

// NOTE: Copied from PoVW.sol. Must be kept in sync.
// TODO: Avoid copying this data type here.
sol! {
    struct WorkLogUpdate {
        address workLogId;
        bytes32 initialCommit;
        bytes32 updatedCommit;
        uint64 updateWork;
    }
}

fn main() {
    // TODO: Implement this guest:
    // 1. Verify one or more claims from the povw-log-builder.
    // 2. Combine the work log updates with the same work log ID into a single update per ID.
    // 3. Verify a signature from the log owner (workLogId) over the final update.
    // 4. ABI encode and commit the results.
    unimplemented!()
}
