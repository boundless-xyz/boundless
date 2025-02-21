// SPDX-License-Identifier: BUSL-1.1
pragma solidity ^0.8.24;

import {IRiscZeroVerifier, Receipt, ReceiptClaim, ReceiptClaimLib} from "risc0/IRiscZeroVerifier.sol";
import {IBoundlessMarketCallback} from "./IBoundlessMarketCallback.sol";

/// @notice Contract for handling proofs delivered by the Boundless Market's callback mechanism
/// @dev This contract provides a framework for applications to safely handle proofs delivered by the Boundless Market, while also
/// providing a path for submitting RISC Zero proofs directly to the application with going via Boundless Market.
///
/// The contract provides two paths:
/// 1. If the `handleProof` callback is called by BoundlessMarket, proof verification is skipped since BoundlessMarket has already
///    verified the proof as part of its fulfillment process.
/// 2. If `handleProof` is called by any other address, the proof is verified using the RISC Zero verifier before
///    proceeding. This provides an alternative path for submitting proofs that is not tightly coupled to the Boundless Market.
///
/// The intention is for developers to inherit the contract and implement the internal `_handleProof` function
/// with their application logic for handling verified proofs.
abstract contract BoundlessMarketCallback is IBoundlessMarketCallback {
    using ReceiptClaimLib for ReceiptClaim;

    IRiscZeroVerifier public immutable VERIFIER;
    address public immutable BOUNDLESS_MARKET;

    /// @notice Initializes the callback contract with verifier and market addresses
    /// @param verifier The RISC Zero verifier contract address
    /// @param boundlessMarket The BoundlessMarket contract address
    constructor(IRiscZeroVerifier verifier, address boundlessMarket) {
        VERIFIER = verifier;
        BOUNDLESS_MARKET = boundlessMarket;
    }

    /// @inheritdoc IBoundlessMarketCallback
    function handleProof(bytes32 imageId, bytes calldata journal, bytes calldata seal) public {
        if (msg.sender == BOUNDLESS_MARKET) {
            _handleProof(imageId, journal, seal);
        } else {
            // Verify the proof before calling callback
            bytes32 claimDigest = ReceiptClaimLib.ok(imageId, sha256(journal)).digest();
            VERIFIER.verify(seal, imageId, claimDigest);
            _handleProof(imageId, journal, seal);
        }
    }

    /// @notice Internal function to be implemented by inheriting contracts
    /// @dev Override this function to implement custom proof handling logic
    /// @param imageId The ID of the RISC Zero guest image that produced the proof
    /// @param journal The output journal from the RISC Zero guest execution
    /// @param seal The cryptographic seal proving correct execution
    function _handleProof(bytes32 imageId, bytes calldata journal, bytes calldata seal) internal virtual;
}
