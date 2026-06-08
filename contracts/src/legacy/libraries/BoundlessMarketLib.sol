// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;

import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";

library BoundlessMarketLib {
    string constant EIP712_DOMAIN = "IBoundlessMarket";
    string constant EIP712_DOMAIN_VERSION = "1";

    /// @notice ABI encode the constructor args for this contract.
    /// @dev This function exists to provide a type-safe way to ABI-encode constructor args, for
    /// use in the deployment process with OpenZeppelin Upgrades. Must be kept in sync with the
    /// signature of the BoundlessMarket constructor.
    function encodeConstructorArgs(
        IRiscZeroVerifier verifier,
        IRiscZeroVerifier applicationVerifier,
        bytes32 assessorId,
        bytes32 deprecatedAssessorId,
        uint32 deprecatedAssessorDuration,
        address stakeTokenContract
    ) internal pure returns (bytes memory) {
        return abi.encode(
            verifier,
            applicationVerifier,
            assessorId,
            deprecatedAssessorId,
            deprecatedAssessorDuration,
            stakeTokenContract
        );
    }
}
