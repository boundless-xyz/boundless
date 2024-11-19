// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.24;

import {Math} from "@openzeppelin/contracts/utils/math/Math.sol";
import {SafeCast} from "@openzeppelin/contracts/utils/math/SafeCast.sol";
import {EIP712Upgradeable} from "@openzeppelin/contracts-upgradeable/utils/cryptography/EIP712Upgradeable.sol";
import {ECDSA} from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import {Ownable2StepUpgradeable} from "@openzeppelin/contracts-upgradeable/access/Ownable2StepUpgradeable.sol";
import {UUPSUpgradeable} from "@openzeppelin/contracts-upgradeable/proxy/utils/UUPSUpgradeable.sol";
import {Initializable} from "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";

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

/// Struct encoding the validated price for a request, intended for use with transient storage.
struct TransientPrice {
    /// Boolean set to true to indicate the request was validated.
    bool valid;
    uint96 price;
}

library TransientPriceLib {
    /// Packs the struct into a uint256.
    function pack(TransientPrice memory x) internal pure returns (uint256) {
        return (uint256(x.valid ? 1 : 0) << 96) | uint256(x.price);
    }

    /// Unpacks the struct from a uint256.
    function unpack(uint256 packed) internal pure returns (TransientPrice memory) {
        return TransientPrice({valid: (packed & (1 << 96)) > 0, price: uint96(packed & uint256(type(uint96).max))});
    }
}

contract ProofMarket is IProofMarket, Initializable, EIP712Upgradeable, Ownable2StepUpgradeable, UUPSUpgradeable {
    using AccountLib for Account;
    using ProofMarketLib for Offer;
    using ProofMarketLib for ProvingRequest;
    using ReceiptClaimLib for ReceiptClaim;
    using SafeCast for uint256;
    using TransientPriceLib for TransientPrice;

    /// @dev The version of the contract.
    uint64 public constant VERSION = 1;

    // Mapping of request ID to lock-in state. Non-zero for requests that are locked in.
    mapping(uint192 => RequestLock) public requestLocks;
    // Mapping of address to account state.
    mapping(address => Account) internal accounts;

    // Using immutable here means the image ID and verifier address is linked to the implementation
    // contract, and not to the proxy. Any deployment that wants to update these values must deploy
    // a new implementation contract.
    /// @custom:oz-upgrades-unsafe-allow state-variable-immutable
    IRiscZeroVerifier public immutable VERIFIER;
    /// @custom:oz-upgrades-unsafe-allow state-variable-immutable
    bytes32 public immutable ASSESSOR_ID;
    string private imageUrl;

    /// In order to fulfill a request, the prover must provide a proof that can be verified with at
    /// most the amount of gas specified by this constant. This requirement exists to ensure the
    /// client can then post the given proof in a new transaction as part of the application.
    uint256 public constant FULFILL_MAX_GAS_FOR_VERIFY = 50000;

    /// @notice When a prover is slashed for failing to fulfill a request, a portion of the stake
    /// is burned, and a portion is sent to the client. This fraction controls that ratio.
    // NOTE: Currently set to burn the entire stake. Can be changed via contract upgrade.
    uint256 public constant SLASHING_BURN_FRACTION_NUMERATOR = 1;
    uint256 public constant SLASHING_BURN_FRACTION_DENOMINATOR = 1;

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor(IRiscZeroVerifier verifier, bytes32 assessorId) {
        VERIFIER = verifier;
        ASSESSOR_ID = assessorId;

        _disableInitializers();
    }

    function initialize(address initialOwner, string calldata _imageUrl) external initializer {
        __Ownable_init(initialOwner);
        __UUPSUpgradeable_init();
        __EIP712_init(ProofMarketLib.EIP712_DOMAIN, ProofMarketLib.EIP712_DOMAIN_VERSION);
        imageUrl = _imageUrl;
    }

    function setImageUrl(string calldata _imageUrl) external onlyOwner {
        imageUrl = _imageUrl;
    }

    function _authorizeUpgrade(address newImplementation) internal override onlyOwner {}

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
        if (accounts[msg.sender].balance < value.toUint96()) {
            revert InsufficientBalance(msg.sender);
        }
        unchecked {
            accounts[msg.sender].balance -= value.toUint96();
        }
        (bool sent,) = msg.sender.call{value: value}("");
        require(sent, "failed to send Ether");
        emit Withdrawal(msg.sender, value);
    }

    // Get the current balance of an account.
    function balanceOf(address addr) public view returns (uint256) {
        return uint256(accounts[addr].balance);
    }

    function submitRequest(ProvingRequest calldata request, bytes calldata clientSignature) external payable {
        if (msg.value > 0) {
            deposit();
        }
        emit RequestSubmitted(request.id, request, clientSignature);
    }

    function lockin(ProvingRequest calldata request, bytes calldata clientSignature) external {
        (address client, uint32 idx) = (ProofMarketLib.requestFrom(request.id), ProofMarketLib.requestIndex(request.id));

        // Recover the prover address and require the client address to equal the address part of the ID.
        bytes32 structHash = _hashTypedDataV4(request.eip712Digest());
        require(ECDSA.recover(structHash, clientSignature) == client, "Invalid client signature");

        _lockinAuthed(request, client, idx, msg.sender);
    }

    function lockinWithSig(
        ProvingRequest calldata request,
        bytes calldata clientSignature,
        bytes calldata proverSignature
    ) external {
        (address client, uint32 idx) = (ProofMarketLib.requestFrom(request.id), ProofMarketLib.requestIndex(request.id));

        // Recover the prover address and require the client address to equal the address part of the ID.
        bytes32 structHash = _hashTypedDataV4(request.eip712Digest());
        address prover = ECDSA.recover(structHash, proverSignature);
        require(ECDSA.recover(structHash, clientSignature) == client, "Invalid client signature");

        _lockinAuthed(request, client, idx, prover);
    }

    /// Check that the request is valid, and not already locked or fulfilled by another prover.
    /// Returns the auction price and deadline for the request.
    function _validateRequestForLockin(ProvingRequest calldata request, address client, uint32 idx)
        internal
        view
        returns (uint96 price, uint64 deadline)
    {
        // Check that the request is internally consistent and is not expired.
        request.offer.requireValid();

        // Check the deadline and compute the current price offered by the reverse Dutch auction.
        price = request.offer.priceAtBlock(uint64(block.number));
        deadline = request.offer.deadline();
        if (deadline < block.number) {
            revert RequestIsExpired({requestId: request.id, deadline: deadline});
        }

        // Check that the request is not already locked or fulfilled.
        // TODO(victor): Currently these checks are run here as part of the priceRequest path.
        // this may be redundant, because we must also check them during fulfillment. Should
        // these checks be moved from this method to _lockinAuthed?
        (bool locked, bool fulfilled) = accounts[client].requestFlags(idx);
        if (locked) {
            revert RequestIsLocked({requestId: request.id});
        }
        if (fulfilled) {
            revert RequestIsFulfilled({requestId: request.id});
        }

        return (price, deadline);
    }

    function _lockinAuthed(ProvingRequest calldata request, address client, uint32 idx, address prover) internal {
        (uint96 price, uint64 deadline) = _validateRequestForLockin(request, client, idx);

        // Deduct funds from the client and prover accounts.
        Account storage clientAccount = accounts[client];
        if (clientAccount.balance < price) {
            revert InsufficientBalance(client);
        }
        Account storage proverAccount = accounts[prover];
        if (proverAccount.balance < request.offer.lockinStake) {
            revert InsufficientBalance(prover);
        }
        unchecked {
            clientAccount.balance -= price;
            proverAccount.balance -= request.offer.lockinStake;
        }

        // Record the lock for the request and emit an event.
        requestLocks[request.id] =
            RequestLock({prover: prover, price: price, deadline: deadline, stake: request.offer.lockinStake});

        clientAccount.setRequestLocked(idx);
        emit RequestLockedin(request.id, prover);
    }

    /// Validates the request and records the price to transient storage such that it can be
    /// fulfilled within the same transaction without taking a lock on it.
    function priceRequest(ProvingRequest calldata request, bytes calldata clientSignature) public {
        (address client, uint32 idx) = (ProofMarketLib.requestFrom(request.id), ProofMarketLib.requestIndex(request.id));

        // Recover the prover address and require the client address to equal the address part of the ID.
        bytes32 structHash = _hashTypedDataV4(request.eip712Digest());
        require(ECDSA.recover(structHash, clientSignature) == client, "Invalid client signature");

        (uint96 price,) = _validateRequestForLockin(request, client, idx);
        uint192 requestId = request.id;

        // Record the price in transient storage, such that the order can be filled in this same transaction.
        // NOTE: Since transient storage is cleared at the end of the transaction, we know that this
        // price will not become stale, and the request cannot expire, while this price is recorded.
        // TODO(#165): Also record a requirements checksum here when solving #165.
        uint256 packed = TransientPrice({valid: true, price: price}).pack();
        assembly {
            tstore(requestId, packed)
        }
    }

    /// Verify the application and assessor receipts, ensuring that the provided fulfillment
    /// satisfies the request.
    // TODO(#165) Return or check the request checksum here.
    function verifyDelivery(Fulfillment calldata fill, bytes calldata assessorSeal, address prover) public view {
        // Verify the application guest proof. We need to verify it here, even though the assessor
        // already verified that the prover has knowledge of a verifying receipt, because we need to
        // make sure the _delivered_ seal is valid.
        bytes32 claimDigest = ReceiptClaimLib.ok(fill.imageId, sha256(fill.journal)).digest();
        VERIFIER.verifyIntegrity{gas: FULFILL_MAX_GAS_FOR_VERIFY}(Receipt(fill.seal, claimDigest));

        // Verify the assessor, which ensures the application proof fulfills a valid request with the given ID.
        // NOTE: Signature checks and recursive verification happen inside the assessor.
        uint192[] memory ids = new uint192[](1);
        ids[0] = fill.id;
        bytes32 assessorJournalDigest = sha256(
            abi.encode(
                AssessorJournal({
                    requestIds: ids,
                    root: claimDigest,
                    eip712DomainSeparator: _domainSeparatorV4(),
                    prover: prover
                })
            )
        );
        // Verification of the assessor seal does not need to comply with FULFILL_MAX_GAS_FOR_VERIFY.
        VERIFIER.verify(assessorSeal, ASSESSOR_ID, assessorJournalDigest);
    }

    /// Verify the application and assessor receipts for the batch, ensuring that the provided
    /// fulfillments satisfy the requests.
    function verifyBatchDelivery(Fulfillment[] calldata fills, bytes calldata assessorSeal, address prover)
        public
        view
    {
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
            abi.encode(
                AssessorJournal({
                    requestIds: ids,
                    root: batchRoot,
                    eip712DomainSeparator: _domainSeparatorV4(),
                    prover: prover
                })
            )
        );
        // Verification of the assessor seal does not need to comply with FULFILL_MAX_GAS_FOR_VERIFY.
        VERIFIER.verify(assessorSeal, ASSESSOR_ID, assessorJournalDigest);
    }

    function fulfill(Fulfillment calldata fill, bytes calldata assessorSeal, address prover) external {
        verifyDelivery(fill, assessorSeal, prover);
        _fulfillVerified(fill.id, prover, fill.requirePayment);

        // TODO(victor): Potentially this should be (re)combined with RequestFulfilled. It would make
        // the logic to watch for a proof a bit more complex, but the gas usage a little less (by
        // about 1000 gas per fulfill based on benchmarks)
        emit ProofDelivered(fill.id, fill.journal, fill.seal);
    }

    function fulfillBatch(Fulfillment[] calldata fills, bytes calldata assessorSeal, address prover) public {
        verifyBatchDelivery(fills, assessorSeal, prover);

        // NOTE: It would be slightly more efficient to keep balances and request flags in memory until a single
        // batch update to storage. However, updating the same storage slot twice only costs 100 gas, so
        // this savings is marginal, and will be outweighed by complicated memory management if not careful.
        for (uint256 i = 0; i < fills.length; i++) {
            _fulfillVerified(fills[i].id, prover, fills[i].requirePayment);

            emit ProofDelivered(fills[i].id, fills[i].journal, fills[i].seal);
        }
    }

    function priceAndFulfillBatch(
        ProvingRequest[] calldata requests,
        bytes[] calldata clientSignatures,
        Fulfillment[] calldata fills,
        bytes calldata assessorSeal,
        address prover
    ) external {
        for (uint256 i = 0; i < requests.length; i++) {
            priceRequest(requests[i], clientSignatures[i]);
        }
        fulfillBatch(fills, assessorSeal, prover);
    }

    /// Complete the fulfillment logic after having verified the app and assessor receipts.
    function _fulfillVerified(uint192 id, address assessorProver, bool requirePayment) internal {
        address client = ProofMarketLib.requestFrom(id);
        uint32 idx = ProofMarketLib.requestIndex(id);

        (bool locked, bool fulfilled) = accounts[client].requestFlags(idx);

        bytes memory paymentError;
        if (locked) {
            paymentError = _fulfillVerifiedLocked(id, client, idx, fulfilled, assessorProver);
        } else {
            paymentError = _fulfillVerifiedUnlocked(id, client, idx, fulfilled, assessorProver);
        }

        if (paymentError.length > 0) {
            if (requirePayment) {
                revertWith(paymentError);
            } else {
                emit PaymentRequirementsFailed(paymentError);
            }
        }
    }

    function _fulfillVerifiedLocked(uint192 id, address client, uint32 idx, bool fulfilled, address assessorProver)
        internal
        returns (bytes memory paymentError)
    {
        RequestLock memory lock = requestLocks[id];

        // Mark the request as fulfilled.
        // NOTE: A request can become fulfilled even if the following checks fail, which control
        // whether payment will be sent. A later transaction can come to transfer the payment.
        if (!fulfilled) {
            accounts[client].setRequestFulfilled(idx);
            emit RequestFulfilled(id);
        }

        // Check pre-conditions for transferring payment.
        if (lock.prover == address(0)) {
            // NOTE: This check is not strictly needed, as the fact that the lock has already been
            // zeroed out means zero value can be transferred. It is provided for clarity.
            return abi.encodeWithSelector(RequestIsFulfilled.selector, id);
        }
        if (lock.deadline < block.number) {
            return abi.encodeWithSelector(RequestIsExpired.selector, id, lock.deadline);
        }
        if (lock.prover != assessorProver) {
            return abi.encodeWithSelector(RequestIsLocked.selector, id);
        }

        // Zero-out the lock to indicate that payment has been delivered and get a bit of a refund on gas.
        requestLocks[id] = RequestLock(address(0), uint96(0), uint64(0), uint96(0));

        accounts[lock.prover].balance += lock.price + lock.stake;
    }

    function _fulfillVerifiedUnlocked(uint192 id, address client, uint32 idx, bool fulfilled, address assessorProver)
        internal
        returns (bytes memory paymentError)
    {
        // When not locked, the fulfilled flag _does_ indicate that payment has already been transferred.
        if (fulfilled) {
            return abi.encodeWithSelector(RequestIsFulfilled.selector, id);
        }

        uint256 packed;
        assembly {
            packed := tload(id)
        }
        TransientPrice memory tprice = TransientPriceLib.unpack(packed);

        // Check that a price has been set, which also requires checks like expiration to pass.
        // NOTE: Maybe "request is not locked or priced" would be more accurate, but seems
        // like that would be a confusing message.
        if (!tprice.valid) {
            return abi.encodeWithSelector(RequestIsNotLocked.selector, id);
        }

        // Mark the request as fulfilled.
        // NOTE: If an unlocked request is fulfilled, but fails the requirements for payment, no
        // payment can ever be rendered for this order.
        Account storage clientAccount = accounts[client];
        clientAccount.setRequestFulfilled(idx);
        emit RequestFulfilled(id);

        // Deduct the funds from client account.
        if (clientAccount.balance < tprice.price) {
            return abi.encodeWithSelector(InsufficientBalance.selector, client);
        }
        unchecked {
            clientAccount.balance -= tprice.price;
        }

        // Pay the prover.
        accounts[assessorProver].balance += tprice.price;
    }

    function slash(uint192 requestId) external {
        address client = ProofMarketLib.requestFrom(requestId);
        uint32 idx = ProofMarketLib.requestIndex(requestId);
        (bool locked,) = accounts[client].requestFlags(idx);

        // Ensure the request is locked, and fetch the lock.
        if (!locked) {
            revert RequestIsNotLocked({requestId: requestId});
        }

        RequestLock memory lock = requestLocks[requestId];

        // If the lock was cleared, the request is already finalized, either by fullfillment or slashing.
        if (lock.prover == address(0)) {
            revert RequestIsNotLocked({requestId: requestId});
        }
        if (lock.deadline >= block.number) {
            revert RequestIsNotExpired({requestId: requestId, deadline: lock.deadline});
        }

        // Zero out the lock to prevent the same request from being slashed twice.
        requestLocks[requestId] = RequestLock(address(0), uint96(0), uint64(0), uint96(0));

        // Calculate the portion of stake that should be burned vs sent to the client.
        // NOTE: If the burn fraction is not properly set, this can overflow.
        // The maximum feasible stake multiplied by the numerator must be less than 2^256.
        uint256 burnValue = uint256(lock.stake) * SLASHING_BURN_FRACTION_NUMERATOR / SLASHING_BURN_FRACTION_DENOMINATOR;
        uint256 transferValue = uint256(lock.stake) - burnValue;

        // Return the price to the client, plus the transfer value. Then burn the burn value.
        accounts[client].balance += lock.price + transferValue.toUint96();
        (bool sent,) = payable(address(0)).call{value: uint256(burnValue)}("");
        require(sent, "Failed to burn Ether");

        emit ProverSlashed(requestId, burnValue, transferValue);
    }

    function imageInfo() external view returns (bytes32, string memory) {
        return (ASSESSOR_ID, imageUrl);
    }

    function submitRootAndFulfillBatch(
        bytes32 root,
        bytes calldata seal,
        Fulfillment[] calldata fills,
        bytes calldata assessorSeal,
        address prover
    ) external {
        // TODO(victor): This will break when we change VERIFIER to point to the router.
        IRiscZeroSetVerifier setVerifier = IRiscZeroSetVerifier(address(VERIFIER));
        setVerifier.submitMerkleRoot(root, seal);
        fulfillBatch(fills, assessorSeal, prover);
    }

    /// Internal utility function to revert with a pre-encoded error.
    function revertWith(bytes memory err) internal pure {
        assembly {
            revert(add(err, 0x20), mload(err))
        }
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
