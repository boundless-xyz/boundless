// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.9;

import {SafeCast} from "openzeppelin/contracts/utils/math/SafeCast.sol";

import {Groth16Verifier} from "./Groth16Verifier.sol";
import {
    IRiscZeroVerifier,
    Output,
    OutputLib,
    Receipt,
    ReceiptClaim,
    ReceiptClaimLib,
    VerificationFailed
} from "risc0/IRiscZeroVerifier.sol";
import {StructHash} from "risc0/StructHash.sol";
import {reverseByteOrderUint256} from "risc0/Util.sol";
import {IRiscZeroSelectable} from "risc0/IRiscZeroSelectable.sol";

/// @notice A Groth16 seal over the claimed receipt claim.
struct Seal {
    uint256[2] a;
    uint256[2][2] b;
    uint256[2] c;
}

/// @notice Error raised when this verifier receives a receipt with a selector that does not match
///         its own. The selector value is calculated from the verifier parameters, and so this
///         usually indicates a mismatch between the version of the prover and this verifier.
error SelectorMismatch(bytes4 received, bytes4 expected);

/// @notice Blake3Groth16 verifier contract for RISC Zero receipts of execution.
contract Blake3Groth16Verifier is IRiscZeroVerifier, IRiscZeroSelectable, Groth16Verifier {
    using ReceiptClaimLib for ReceiptClaim;
    using OutputLib for Output;
    using SafeCast for uint256;

    /// @notice Semantic version of the RISC Zero system of which this contract is part.
    /// @dev This is set to be equal to the version of the risc0-zkvm crate.
    string public constant VERSION = "0.0.1";

    /// @notice Control root hash binding the set of circuits in the RISC Zero system.
    /// @dev This value controls what set of recursion programs (e.g. lift, join, resolve), and
    /// therefore what version of the zkVM circuit, will be accepted by this contract. Each
    /// instance of this verifier contract will accept a single release of the RISC Zero circuits.
    ///
    /// New releases of RISC Zero's zkVM require updating these values. These values can be
    /// calculated from the [risc0 monorepo][1] using: `cargo xtask bootstrap`.
    ///
    /// [1]: https://github.com/risc0/risc0
    bytes16 public immutable CONTROL_ROOT_0;
    bytes16 public immutable CONTROL_ROOT_1;
    bytes32 public immutable BN254_CONTROL_ID;

    /// @notice A short key attached to the seal to select the correct verifier implementation.
    /// @dev The selector is taken from the hash of the verifier parameters including the Groth16
    ///      verification key and the control IDs that commit to the RISC Zero circuits. If two
    ///      receipts have different selectors (i.e. different verifier parameters), then it can
    ///      generally be assumed that they need distinct verifier implementations. This is used as
    ///      part of the RISC Zero versioning mechanism.
    ///
    ///      A selector is not intended to be collision resistant, in that it is possible to find
    ///      two preimages that result in the same selector. This is acceptable since it's purpose
    ///      to a route a request among a set of trusted verifiers, and to make errors of sending a
    ///      receipt to a mismatching verifiers easier to debug. It is analogous to the ABI
    ///      function selectors.
    bytes4 public immutable SELECTOR;

    /// @notice Identifier for the Groth16 verification key encoded into the base contract.
    /// @dev This value is computed at compile time.
    function verifierKeyDigest() internal pure returns (bytes32) {
        bytes32[] memory icDigests = new bytes32[](2);
        icDigests[0] = sha256(abi.encodePacked(IC0x, IC0y));
        icDigests[1] = sha256(abi.encodePacked(IC1x, IC1y));

        return sha256(
            abi.encodePacked(
                // tag
                sha256("risc0_groth16.VerifyingKey"),
                // down
                sha256(abi.encodePacked(alphax, alphay)),
                sha256(abi.encodePacked(betax1, betax2, betay1, betay2)),
                sha256(abi.encodePacked(gammax1, gammax2, gammay1, gammay2)),
                sha256(abi.encodePacked(deltax1, deltax2, deltay1, deltay2)),
                StructHash.taggedList(sha256("risc0_groth16.VerifyingKey.IC"), icDigests),
                // down length
                uint16(5) << 8
            )
        );
    }

    constructor(bytes32 controlRoot, bytes32 bn254ControlId) {
        (CONTROL_ROOT_0, CONTROL_ROOT_1) = splitDigest(controlRoot);
        BN254_CONTROL_ID = bn254ControlId;

        SELECTOR = bytes4(
            sha256(
                abi.encodePacked(
                    // tag
                    sha256("risc0.Groth16ReceiptVerifierParameters"),
                    // down
                    controlRoot,
                    reverseByteOrderUint256(uint256(bn254ControlId)),
                    verifierKeyDigest(),
                    // down length
                    uint16(3) << 8
                )
            )
        );
    }

    /// @notice splits a digest into two 128-bit halves to use as public signal inputs.
    /// @dev RISC Zero's Circom verifier circuit takes each of two hash digests in two 128-bit
    /// chunks. These values can be derived from the digest by splitting the digest in half and
    /// then reversing the bytes of each.
    function splitDigest(bytes32 digest) internal pure returns (bytes16, bytes16) {
        uint256 reversed = reverseByteOrderUint256(uint256(digest));
        return (bytes16(uint128(reversed)), bytes16(uint128(reversed >> 128)));
    }

    /// @inheritdoc IRiscZeroVerifier
    function verify(bytes calldata seal, bytes32 imageId, bytes32 journalDigest) external pure {
        seal;
        imageId;
        journalDigest;
        revert("Use verifyIntegrity");
    }

    /// @inheritdoc IRiscZeroVerifier
    function verifyIntegrity(Receipt calldata receipt) external view {
        return _verifyIntegrity(receipt.seal, receipt.claimDigest);
    }

    /// @notice internal implementation of verifyIntegrity, factored to avoid copying calldata bytes to memory.
    function _verifyIntegrity(bytes calldata seal, bytes32 claimDigest) internal view {
        // Check that the seal has a matching selector. Mismatch generally indicates that the
        // prover and this verifier are using different parameters, and so the verification
        // will not succeed.
        if (SELECTOR != bytes4(seal[:4])) {
            revert SelectorMismatch({received: bytes4(seal[:4]), expected: SELECTOR});
        }

        // Run the Groth16 verify procedure.
        Seal memory decodedSeal = abi.decode(seal[4:], (Seal));
        bool verified = this.verifyProof(
            decodedSeal.a,
            decodedSeal.b,
            decodedSeal.c,
            [
                /// Blake3(Sha256(control_root, pre_state_digest, post_state_digest, id_bn254_fr), journal)[:31]
                uint256(claimDigest)
            ]
        );

        // Revert is verification failed.
        if (!verified) {
            revert VerificationFailed();
        }
    }
}
