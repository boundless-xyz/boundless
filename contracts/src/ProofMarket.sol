// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

import {Math} from "@openzeppelin/contracts/utils/math/Math.sol";
import {SafeCast} from "@openzeppelin/contracts/utils/math/SafeCast.sol";
import {EIP712} from "@openzeppelin/contracts/utils/cryptography/EIP712.sol";
import {ECDSA} from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import {IRiscZeroVerifier, Receipt, ReceiptClaim, ReceiptClaimLib} from "risc0/IRiscZeroVerifier.sol";
import {IRiscZeroSetVerifier} from "./IRiscZeroSetVerifier.sol";

import {IProofMarket, ProvingRequest, Offer, Fulfillment, AssessorJournal} from "./IProofMarket.sol";
import {ProofMarketLib} from "./ProofMarketLib.sol";

// TODO(#165): A potential issue with the current approach is: if the client
// signs a request with a given ID, it expires with no bids, and they sign a
// new request with the same ID and different requirements, which gets a
// lockin, the prover is not actually bound to fulfill the one that was locked
// in. A solution that avoids increasing state usage would be to add a deadline
// check in the assessor. If two requests are valid in the same window with
// the same ID, this is still an issue. However this is much more reasonably
// considered a mistake, and client implementations should avoid it. Simplest
// way for a client to avoid this issue would be always increment their index,
// and let unfulfilled request IDs go unused... this of course requires client
// state. Another, would be use scanning to determine the first unused ID. A
// third approach would be to expand the request lock, but only store it's
// digest in state. With this approach, we can include more info in it. This
// would probably also expand the journal for the assessor. None of these
// are perfect.

uint256 constant REQUEST_FLAGS_BITWIDTH = 2;

/// @notice Account state is a combination of the account balance, and locked and fulfilled flags for requests.
struct Account {
    /// @dev uint96 is enough to represent the entire token supply of Ether.
    uint96 balance;
    /// @notice 80 pairs of 2 bits representing the status of a request. One bit is for lock-in and
    /// the other is for fulfillment.
    /// @dev Request state flags are packed into a uint160 to make balance and flags for the first
    /// 80 requests fit in one slot.
    uint160 requestFlagsInitial;
    /// @dev Flags for the remaining requests are in a storage array. Each uint256 holds the packed
    /// flags for 128 requests, indexed in a linear fashion. Note that this struct cannot be
    /// instantiated in memory.
    uint256[(1 << 32) * REQUEST_FLAGS_BITWIDTH / 256] requestFlagsExtended;
}

library AccountLib {
    /// Gets the locked and fulfilled request flags for the request with the given index.
    function requestFlags(Account storage account, uint32 idx) internal view returns (bool locked, bool fulfilled) {
        if (idx < 160 / REQUEST_FLAGS_BITWIDTH) {
            uint160 masked = (
                account.requestFlagsInitial
                    & (uint160((1 << REQUEST_FLAGS_BITWIDTH) - 1) << uint160(idx * REQUEST_FLAGS_BITWIDTH))
            ) >> (idx * REQUEST_FLAGS_BITWIDTH);
            return (masked & uint160(1) != 0, masked & uint160(2) != 0);
        } else {
            uint256 idxShifted = idx - (160 / REQUEST_FLAGS_BITWIDTH);
            uint256 packed = account.requestFlagsExtended[(idxShifted * REQUEST_FLAGS_BITWIDTH) / 256];
            uint256 maskShift = (idxShifted * REQUEST_FLAGS_BITWIDTH) % 256;
            uint256 masked = (packed & (uint256((1 << REQUEST_FLAGS_BITWIDTH) - 1) << maskShift)) >> maskShift;
            return (masked & uint256(1) != 0, masked & uint256(2) != 0);
        }
    }

    /// @notice Sets the locked and fulfilled request flags for the request with the given index.
    /// @dev The given value of flags will be applied with |= to the flags for the request.
    /// Least significant bit is locked, second-least significant is fulfilled.
    function setRequestFlags(Account storage account, uint32 idx, uint8 flags) internal {
        assert(flags < (1 << REQUEST_FLAGS_BITWIDTH));
        if (idx < 160 / REQUEST_FLAGS_BITWIDTH) {
            uint160 mask = uint160(flags) << uint160(idx * REQUEST_FLAGS_BITWIDTH);
            account.requestFlagsInitial |= mask;
        } else {
            uint256 idxShifted = idx - (160 / REQUEST_FLAGS_BITWIDTH);
            uint256 mask = uint256(flags) << (uint256(idxShifted * REQUEST_FLAGS_BITWIDTH) % 256);
            account.requestFlagsExtended[(idxShifted * REQUEST_FLAGS_BITWIDTH) / 256] |= mask;
        }
    }

    function setRequestLocked(Account storage account, uint32 idx) internal {
        setRequestFlags(account, idx, 1);
    }

    function setRequestFulfilled(Account storage account, uint32 idx) internal {
        setRequestFlags(account, idx, 2);
    }
}

struct RequestLock {
    address prover;
    uint96 price;
    uint64 deadline;
    // Prover stake that may be taken if a proof is not delivered by the deadline.
    uint96 stake;
}

contract ProofMarket is IProofMarket, EIP712 {
    using AccountLib for Account;
    using ProofMarketLib for Offer;
    using ProofMarketLib for ProvingRequest;
    using ReceiptClaimLib for ReceiptClaim;
    using SafeCast for uint256;

    // Mapping of request ID to lock-in state. Non-zero for requests that are locked in.
    mapping(uint192 => RequestLock) public requestLocks;
    // Mapping of address to account state.
    mapping(address => Account) internal accounts;

    // NOTE: Expected to be a verifier such at the RiscZeroSetVerifier where
    // verifying individual claims is relatively cheap.
    IRiscZeroVerifier public immutable VERIFIER;
    bytes32 public immutable ASSESSOR_ID;
    string private imageUrl;

    /// In order to fulfill a request, the prover must provide a proof that can be verified with at
    /// most the amount of gas specified by this constant. This requirement exists to ensure the
    /// client can then post the given proof in a new transaction as part of the application.
    uint256 public constant FULFILL_MAX_GAS_FOR_VERIFY = 50000;

    constructor(IRiscZeroVerifier verifier, bytes32 assessorId, string memory _imageUrl)
        EIP712(ProofMarketLib.EIP712_DOMAIN, ProofMarketLib.EIP712_DOMAIN_VERSION)
    {
        VERIFIER = verifier;
        ASSESSOR_ID = assessorId;
        imageUrl = _imageUrl;
    }

    function requestIsFulfilled(uint192 id) external view returns (bool) {
        (, bool fulfilled) = accounts[address(uint160(id >> 32))].requestFlags(uint32(id));
        return fulfilled;
    }

    function requestIsLocked(uint192 id) external view returns (bool) {
        (bool locked,) = accounts[address(uint160(id >> 32))].requestFlags(uint32(id));
        return locked;
    }

    function requestDeadline(uint192 id) external view returns (uint64) {
        return requestLocks[id].deadline;
    }

    /// Domain separator for producing an EIP-712 signature to be verified by this contract.
    function eip712DomainSeparator() external view returns (bytes32) {
        return _domainSeparatorV4();
    }

    // Deposit Ether into the market.
    function deposit() public payable {
        accounts[msg.sender].balance += msg.value.toUint96();
        emit Deposit(msg.sender, msg.value);
    }

    // Withdraw Ether from the market.
    function withdraw(uint256 value) public {
        accounts[msg.sender].balance -= value.toUint96();
        (bool sent,) = msg.sender.call{value: value}("");
        require(sent, "failed to send Ether");
        emit Withdrawal(msg.sender, value);
    }

    // Get the current balance of an account.
    function balanceOf(address addr) public view returns (uint256) {
        return uint256(accounts[addr].balance);
    }

    function submitRequest(ProvingRequest calldata request, bytes memory clientSignature) external payable {
        accounts[msg.sender].balance += msg.value.toUint96();
        emit RequestSubmitted(request, clientSignature);
    }

    function lockin(ProvingRequest calldata request, bytes memory clientSignature) external {
        (address client, uint32 idx) = (ProofMarketLib.requestFrom(request.id), ProofMarketLib.requestIndex(request.id));

        // Recover the prover address and require the client address to equal the address part of the ID.
        bytes32 structHash = _hashTypedDataV4(request.eip712Digest());
        require(ECDSA.recover(structHash, clientSignature) == client, "Invalid client signature");

        _lockinAuthed(request, client, idx, msg.sender);
    }

    function lockinWithSig(
        ProvingRequest calldata request,
        bytes memory clientSignature,
        bytes calldata proverSignature
    ) external {
        (address client, uint32 idx) = (ProofMarketLib.requestFrom(request.id), ProofMarketLib.requestIndex(request.id));

        // Recover the prover address and require the client address to equal the address part of the ID.
        bytes32 structHash = _hashTypedDataV4(request.eip712Digest());
        address prover = ECDSA.recover(structHash, proverSignature);
        require(ECDSA.recover(structHash, clientSignature) == client, "Invalid client signature");

        _lockinAuthed(request, client, idx, prover);
    }

    function _lockinAuthed(ProvingRequest calldata request, address client, uint32 idx, address prover) internal {
        // Check that the request is internally consistent and is not expired.
        request.offer.requireValid();

        // We are ending the reverse Dutch auction at the current price.
        uint96 price = request.offer.priceAtBlock(uint64(block.number));
        uint64 deadline = request.offer.deadline();
        if (deadline < block.number) {
            revert RequestIsExpired({requestId: request.id, deadline: deadline});
        }

        // Check that the request is not already locked or fulfilled.
        (bool locked, bool fulfilled) = accounts[client].requestFlags(idx);
        if (locked) {
            revert RequestIsLocked({requestId: request.id});
        }
        if (fulfilled) {
            revert RequestIsFulfilled({requestId: request.id});
        }

        // Lock the request such that only the given prover can fulfill it (or else face a penalty).
        Account storage clientAccount = accounts[client];
        clientAccount.setRequestLocked(idx);
        requestLocks[request.id] =
            RequestLock({prover: prover, price: price, deadline: deadline, stake: request.offer.lockinStake});

        // Deduct the funds from client and prover accounts.
        if (clientAccount.balance < price) {
            revert InsufficientBalance(client);
        }
        Account storage proverAccount = accounts[prover];
        if (proverAccount.balance < request.offer.lockinStake) {
            revert InsufficientBalance(prover);
        }

        // TODO: Double check this is properly covered by the above reverts
        unchecked {
            clientAccount.balance -= price;
            proverAccount.balance -= request.offer.lockinStake;
        }
        accounts[client] = clientAccount;

        emit RequestLockedin(request.id, prover);
    }

    // TODO(victor): Add a path that allows a prover to fuilfill a request without first sending a lock-in.
    function fulfill(Fulfillment calldata fill, bytes calldata assessorSeal) external {
        // Verify the application guest proof. We need to verify it here, even though the market
        // guest already verified that the prover has knowledge of a verifying receipt, because
        // we need to make sure the _delivered_ seal is valid.
        // TODO(victor): Support journals hashed with keccak instead of SHA-256.
        bytes32 claimDigest = ReceiptClaimLib.ok(fill.imageId, sha256(fill.journal)).digest();
        VERIFIER.verifyIntegrity{gas: FULFILL_MAX_GAS_FOR_VERIFY}(Receipt(fill.seal, claimDigest));

        // Verify the assessor, which ensures the application proof fulfills a valid request with the given ID.
        // NOTE: Signature checks and recursive verification happen inside the assessor.
        uint192[] memory ids = new uint192[](1);
        ids[0] = fill.id;
        bytes32 assessorJournalDigest = sha256(
            abi.encode(
                AssessorJournal({requestIds: ids, root: claimDigest, eip712DomainSeparator: _domainSeparatorV4()})
            )
        );
        // Verification of the assessor seal does not need to comply with FULFILL_MAX_GAS_FOR_VERIFY.
        VERIFIER.verify(assessorSeal, ASSESSOR_ID, assessorJournalDigest);

        _fulfillVerified(fill.id);

        emit RequestFulfilled(fill.id, fill.journal, fill.seal);
    }

    function fulfillBatch(Fulfillment[] calldata fills, bytes calldata assessorSeal) public {
        // TODO(victor): Figure out how much the memory here is costing. If it's significant, we can do some tricks to reduce memory pressure.
        bytes32[] memory claimDigests = new bytes32[](fills.length);
        uint192[] memory ids = new uint192[](fills.length);
        for (uint256 i = 0; i < fills.length; i++) {
            ids[i] = fills[i].id;
            claimDigests[i] = ReceiptClaimLib.ok(fills[i].imageId, sha256(fills[i].journal)).digest();
            VERIFIER.verifyIntegrity{gas: FULFILL_MAX_GAS_FOR_VERIFY}(Receipt(fills[i].seal, claimDigests[i]));
        }
        bytes32 batchRoot = MerkleProofish.processTree(claimDigests);

        // Verify the assessor, which ensures the application proof fulfills a valid request with the given ID.
        // NOTE: Signature checks and recursive verification happen inside the assessor.
        bytes32 assessorJournalDigest = sha256(
            abi.encode(AssessorJournal({requestIds: ids, root: batchRoot, eip712DomainSeparator: _domainSeparatorV4()}))
        );
        // Verification of the assessor seal does not need to comply with FULFILL_MAX_GAS_FOR_VERIFY.
        VERIFIER.verify(assessorSeal, ASSESSOR_ID, assessorJournalDigest);

        // NOTE: It would be slightly more efficient to keep balances and request flags in memory until a single
        // batch update to storage. However, updating the the same storage slot twice only costs 100 gas, so
        // this savings is marginal, and will be outweighed by complicated memory management if not careful.
        for (uint256 i = 0; i < fills.length; i++) {
            _fulfillVerified(ids[i]);

            emit RequestFulfilled(fills[i].id, fills[i].journal, fills[i].seal);
        }
    }

    /// Complete the fulfillment logic after having verified the app and assessor receipts.
    function _fulfillVerified(uint192 id) internal {
        address client = ProofMarketLib.requestFrom(id);
        uint32 idx = ProofMarketLib.requestIndex(id);

        // Check that the request is not locked to a different prover.
        (bool locked, bool fulfilled) = accounts[client].requestFlags(idx);

        // Ensure the request is locked, and fetch the lock.
        if (!locked) {
            revert RequestIsNotLocked({requestId: id});
        }
        if (fulfilled) {
            revert RequestIsFulfilled({requestId: id});
        }

        RequestLock memory lock = requestLocks[id];

        if (lock.deadline < block.number) {
            revert RequestIsExpired({requestId: id, deadline: lock.deadline});
        }

        // Zero out the lock to get a bit of a refund on gas.
        requestLocks[id] = RequestLock(address(0), uint96(0), uint64(0), uint96(0));

        // Mark the request as fulfilled and pay the prover.
        accounts[client].setRequestFulfilled(idx);
        accounts[lock.prover].balance += lock.price + lock.stake;
    }

    function slash(uint192 requestId) external {
        address client = ProofMarketLib.requestFrom(requestId);
        uint32 idx = ProofMarketLib.requestIndex(requestId);
        (bool locked, bool fulfilled) = accounts[client].requestFlags(idx);

        // Ensure the request is locked, and fetch the lock.
        if (!locked) {
            revert RequestIsNotLocked({requestId: requestId});
        }
        if (fulfilled) {
            revert RequestIsFulfilled({requestId: requestId});
        }

        RequestLock memory lock = requestLocks[requestId];

        if (lock.deadline >= block.number) {
            revert RequestIsNotExpired({requestId: requestId, deadline: lock.deadline});
        }

        if (lock.prover == address(0)) {
            revert RequestAlreadySlashed({requestId: requestId});
        }

        // Zero out the lock to prevent the same request from being slashed twice.
        requestLocks[requestId] = RequestLock(address(0), uint96(0), uint64(0), uint96(0));

        emit LockinStakeBurned(requestId, lock.stake);

        // Return the price to the client and burn the stake.
        accounts[client].balance += lock.price;
        (bool sent,) = payable(address(0)).call{value: uint256(lock.stake)}("");
        require(sent, "Failed to burn Ether");
    }

    function imageInfo() external view returns (bytes32, string memory) {
        return (ASSESSOR_ID, imageUrl);
    }

    function submitRootAndFulfillBatch(
        bytes32 root,
        bytes calldata seal,
        Fulfillment[] calldata fills,
        bytes calldata assessorSeal
    ) external {
        IRiscZeroSetVerifier setVerifier = IRiscZeroSetVerifier(address(VERIFIER));
        setVerifier.submitMerkleRoot(root, seal);
        fulfillBatch(fills, assessorSeal);
    }
}

// Functions copied from OZ MerkleProof library to allow building the Merkle tree above.
// TODO(victor): Drop this library.
library MerkleProofish {
    // Compute the root of the Merkle tree given all of its leaves.
    // Assumes that the array of leaves is no longer needed, and can be overwritten.
    function processTree(bytes32[] memory leaves) internal pure returns (bytes32 root) {
        require(leaves.length > 0, "Leaves array must contain at least one element");

        // If there's only one leaf, the root is the leaf itself
        if (leaves.length == 1) {
            return leaves[0];
        }

        uint256 n = leaves.length;

        // Process the leaves array in pairs, iteratively computing the hash of each pair
        while (n > 1) {
            uint256 nextLevelLength = (n + 1) / 2; // Upper bound of next level (handles odd number of elements)

            // Hash the current level's pairs and place results at the start of the array
            for (uint256 i = 0; i < n / 2; i++) {
                leaves[i] = _hashPair(leaves[2 * i], leaves[2 * i + 1]);
            }

            // If there's an odd number of elements, propagate the last element directly
            if (n % 2 == 1) {
                leaves[n / 2] = leaves[n - 1];
            }

            // Move to the next level (the computed hashes are now the new "leaves")
            n = nextLevelLength;
        }

        // The root is now the single element left in the array
        root = leaves[0];
    }

    /**
     * @dev Sorts the pair (a, b) and hashes the result.
     */
    function _hashPair(bytes32 a, bytes32 b) internal pure returns (bytes32) {
        return a < b ? _efficientHash(a, b) : _efficientHash(b, a);
    }

    /**
     * @dev Implementation of keccak256(abi.encode(a, b)) that doesn't allocate or expand memory.
     */
    function _efficientHash(bytes32 a, bytes32 b) private pure returns (bytes32 value) {
        /// @solidity memory-safe-assembly
        assembly {
            mstore(0x00, a)
            mstore(0x20, b)
            value := keccak256(0x00, 0x40)
        }
    }
}
