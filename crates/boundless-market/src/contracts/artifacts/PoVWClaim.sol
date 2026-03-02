// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
pragma solidity ^0.8.26;

/// @title PoVW Claim Struct
/// @notice Records the number of zkVM cycles attributed to a fill via Proof-of-Verifiable-Work.
/// @dev Emitted as a dense array in AssessorJournal/AssessorReceipt, parallel to the fills array.
/// An entry with cycleCount = 0 means no PoVW attribution for that fill.
struct PoVWClaim {
    /// @notice Number of zkVM cycles attributed to this fill.
    uint64 cycleCount;
}
