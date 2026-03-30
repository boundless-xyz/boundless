// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.9;

import {IRiscZeroVerifier, Receipt} from "risc0/IRiscZeroVerifier.sol";
import {RiscZeroVerifierRouter} from "./RiscZeroVerifierRouter.sol";

/// @notice A layered router enabling additional verifier implementations to be registered on top of a
///         parent router, while delegating unknown selectors to the parent.
/// @dev Resolution checks this router first and falls back to the parent router when unset.
contract VerifierLayeredRouter is RiscZeroVerifierRouter {
    /// @notice The parent RISC Zero verifier router used as fallback.
    RiscZeroVerifierRouter public immutable parentRouter;

    constructor(address owner, RiscZeroVerifierRouter _parentRouter) RiscZeroVerifierRouter(owner) {
        require(address(_parentRouter) != address(0), "Parent router address cannot be zero");
        parentRouter = _parentRouter;
    }

    /// @notice Gets the parent RISC Zero verifier router.
    function getParentRouter() external view returns (RiscZeroVerifierRouter) {
        return parentRouter;
    }

    /// @notice Adds a verifier to the router, such that it can receive receipt verification calls.
    /// @dev Ensures that the selector is not already registered or removed in either this router or the parent router.
    function addVerifier(bytes4 selector, IRiscZeroVerifier verifier) external override onlyOwner {
        // Ensure the selector is not removed from the parent router.
        if (parentRouter.verifiers(selector) == TOMBSTONE) {
            revert SelectorRemoved({selector: selector});
        }
        // Ensure the selector is not already in use in the parent router.
        if (parentRouter.verifiers(selector) != UNSET) {
            revert SelectorInUse({selector: selector});
        }
        // Ensure the selector is not removed from this router.
        if (verifiers[selector] == TOMBSTONE) {
            revert SelectorRemoved({selector: selector});
        }
        // Ensure the selector is not already in use in this router.
        if (verifiers[selector] != UNSET) {
            revert SelectorInUse({selector: selector});
        }
        // Ensure the verifier address is not zero.
        if (address(verifier) == address(0)) {
            revert VerifierAddressZero();
        }
        verifiers[selector] = verifier;
    }

    /// @inheritdoc RiscZeroVerifierRouter
    function removeVerifier(bytes4 selector) external override onlyOwner {
        verifiers[selector] = TOMBSTONE;
    }

    /// @notice Get the associated verifier, falling back to the parent router if unset.
    function getVerifier(bytes4 selector) public view override returns (IRiscZeroVerifier) {
        IRiscZeroVerifier verifier = verifiers[selector];
        // If the verifier is unset, fall back to the parent router.
        if (verifier == UNSET) {
            return parentRouter.getVerifier(selector);
        }
        if (verifier == TOMBSTONE) {
            revert SelectorRemoved({selector: selector});
        }
        return verifier;
    }

    /// @inheritdoc IRiscZeroVerifier
    function verify(bytes calldata seal, bytes32 imageId, bytes32 journalDigest) external view override {
        bytes4 selector = bytes4(seal[0:4]);
        IRiscZeroVerifier v = verifiers[selector];

        if (v == UNSET) {
            // Single external call to parent (it resolves + forwards)
            parentRouter.verify(seal, imageId, journalDigest);
            return;
        }
        if (v == TOMBSTONE) {
            revert SelectorRemoved({selector: selector});
        }
        v.verify(seal, imageId, journalDigest);
    }

    /// @inheritdoc IRiscZeroVerifier
    function verifyIntegrity(Receipt calldata receipt) external view override {
        bytes4 selector = bytes4(receipt.seal[0:4]);
        IRiscZeroVerifier v = verifiers[selector];

        if (v == UNSET) {
            parentRouter.verifyIntegrity(receipt);
            return;
        }
        if (v == TOMBSTONE) {
            revert SelectorRemoved({selector: selector});
        }
        v.verifyIntegrity(receipt);
    }
}
