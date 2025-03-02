// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
pragma solidity ^0.8.24;

import {IRiscZeroVerifier, Receipt, ReceiptClaim, ReceiptClaimLib} from "risc0/IRiscZeroVerifier.sol";
import {IBoundlessMarketCallback} from "./IBoundlessMarketCallback.sol";

/// @notice Contract for handling proofs delivered by the Boundless Market's callback mechanism.
/// @dev This contract provides a framework for applications to safely handle proofs delivered by the Boundless Market for a specific image ID.
/// The intention is for developers to inherit the contract and implement the internal `_handleProof` function.
/// @dev We verify the proof here even though the Boundless Market already verifies the proof as part of its fulfillment process.
/// This allows proofs to be submitted directly to the application without going through the Boundless Market.
abstract contract BoundlessMarketCallback is IBoundlessMarketCallback {
    using ReceiptClaimLib for ReceiptClaim;

    IRiscZeroVerifier public immutable VERIFIER;
    address public immutable BOUNDLESS_MARKET;
    bytes32 public immutable IMAGE_ID;
    
    /// @notice Initializes the callback contract with verifier and market addresses
    /// @param verifier The RISC Zero verifier contract address
    /// @param boundlessMarket The BoundlessMarket contract address
    /// @param imageId The image ID to accept proofs of.
    constructor(IRiscZeroVerifier verifier, address boundlessMarket, bytes32 imageId) {
        VERIFIER = verifier;
        BOUNDLESS_MARKET = boundlessMarket;
        IMAGE_ID = imageId;
    }

    /// @inheritdoc IBoundlessMarketCallback
    function handleProof(bytes32 imageId, bytes calldata journal, bytes calldata seal) public {
        require(msg.sender == BOUNDLESS_MARKET, "Invalid sender");
        require(imageId == IMAGE_ID, "Invalid Image ID");
        // Verify the proof before calling callback
        bytes32 claimDigest = ReceiptClaimLib.ok(imageId, sha256(journal)).digest();
        VERIFIER.verifyIntegrity(Receipt(seal, claimDigest));
        _handleProof(imageId, journal, seal);
    }

    /// @notice Internal function to be implemented by inheriting contracts
    /// @dev Override this function to implement custom proof handling logic
    /// @param imageId The ID of the RISC Zero guest image that produced the proof
    /// @param journal The output journal from the RISC Zero guest execution
    /// @param seal The cryptographic seal proving correct execution
    function _handleProof(bytes32 imageId, bytes calldata journal, bytes calldata seal) internal virtual;
}
