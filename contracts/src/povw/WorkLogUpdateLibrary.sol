// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
pragma solidity ^0.8.26;

import {WorkLogUpdate} from "./IPovwAccounting.sol";

library WorkLogUpdateLibrary {
    string constant WORK_LOG_UPDATE_TYPE =
        "WorkLogUpdate(address workLogId,bytes32 initialCommit,bytes32 updatedCommit,uint64 updateValue,address valueRecipient)";

    bytes32 constant WORK_LOG_UPDATE_TYPEHASH = keccak256(abi.encodePacked(WORK_LOG_UPDATE_TYPE));

    /// @notice Computes the EIP-712 digest for the given WorkLogUpdate.
    /// @param update The WorkLogUpdate to compute the digest for.
    /// @return The EIP-712 digest of the WorkLogUpdate.
    function eip712Digest(WorkLogUpdate memory update) internal pure returns (bytes32) {
        return keccak256(
            abi.encode(
                WORK_LOG_UPDATE_TYPEHASH,
                update.workLogId,
                update.initialCommit,
                update.updatedCommit,
                update.updateValue,
                update.valueRecipient
            )
        );
    }
}
