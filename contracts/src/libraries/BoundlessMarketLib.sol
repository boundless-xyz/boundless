// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.20;

import {IRiscZeroVerifier} from "risc0/IRiscZeroVerifier.sol";

library BoundlessMarketLib {
    string constant EIP712_DOMAIN = "IBoundlessMarket";
    string constant EIP712_DOMAIN_VERSION = "1";

    /// @notice ABI encode the constructor args for this contract.
    /// @dev This function exists to provide a type-safe way to ABI-encode constructor args, for
    /// use in the deployment process with OpenZeppelin Upgrades. Must be kept in sync with the
    /// signature of the BoundlessMarket constructor.
    function encodeConstructorArgs(IRiscZeroVerifier verifier, bytes32 assessorId, address stakeTokenContract)
        internal
        pure
        returns (bytes memory)
    {
        return abi.encode(verifier, assessorId, stakeTokenContract);
    }
}
