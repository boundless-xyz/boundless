// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.24;

import {Math} from "@openzeppelin/contracts/utils/math/Math.sol";
import {SafeCast} from "@openzeppelin/contracts/utils/math/SafeCast.sol";
import {ECDSA} from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import {EIP712Upgradeable} from "@openzeppelin/contracts-upgradeable/utils/cryptography/EIP712Upgradeable.sol";
import {Ownable2StepUpgradeable} from "@openzeppelin/contracts-upgradeable/access/Ownable2StepUpgradeable.sol";
import {UUPSUpgradeable} from "@openzeppelin/contracts-upgradeable/proxy/utils/UUPSUpgradeable.sol";
import {Initializable} from "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {ERC20Burnable} from "@openzeppelin/contracts/token/ERC20/extensions/ERC20Burnable.sol";
import {IERC20Permit} from "@openzeppelin/contracts/token/ERC20/extensions/IERC20Permit.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";

import {IRiscZeroVerifier, Receipt, ReceiptClaim, ReceiptClaimLib} from "risc0/IRiscZeroVerifier.sol";
import {IRiscZeroSetVerifier} from "risc0/IRiscZeroSetVerifier.sol";

import {IBoundlessMarket} from "./IBoundlessMarket.sol";
import {Account} from "./types/Account.sol";
import {AssessorJournal} from "./types/AssessorJournal.sol";
import {Fulfillment} from "./types/Fulfillment.sol";

import {FulfillmentAssessor} from "./types/FulfillmentAssessor.sol";
import {ProofRequest} from "./types/ProofRequest.sol";
import {RequestId} from "./types/RequestId.sol";
import {RequestLock} from "./types/RequestLock.sol";
import {Selector, Selectors} from "./types/Selector.sol";
import {TransientPrice, TransientPriceLibrary} from "./types/TransientPrice.sol";

import {BoundlessMarketLib} from "./libraries/BoundlessMarketLib.sol";
import {MerkleProofish} from "./libraries/MerkleProofish.sol";

contract BoundlessMarket is
    IBoundlessMarket,
    Initializable,
    EIP712Upgradeable,
    Ownable2StepUpgradeable,
    UUPSUpgradeable
{
    using ReceiptClaimLib for ReceiptClaim;
    using SafeCast for uint256;
    using SafeERC20 for IERC20;

    /// @dev The version of the contract, with respect to upgrades.
    uint64 public constant VERSION = 1;

    // Mapping of request ID to lock-in state. Non-zero for requests that are locked in.
    mapping(RequestId => RequestLock) public requestLocks;
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
    /// @custom:oz-upgrades-unsafe-allow state-variable-immutable
    address public immutable STAKE_TOKEN_CONTRACT;

    /// In order to fulfill a request, the prover must provide a proof that can be verified with at
    /// most the amount of gas specified by this constant. This requirement exists to ensure the
    /// client can then post the given proof in a new transaction as part of the application.
    uint256 public constant FULFILL_MAX_GAS_FOR_VERIFY = 50000;

    /// @notice When a prover is slashed for failing to fulfill a request, a portion of the stake
    /// is burned, and a portion is sent to the client. This fraction controls that ratio.
    // NOTE: Currently set to burn the entire stake. Can be changed via contract upgrade.
    uint256 public constant SLASHING_BURN_FRACTION_NUMERATOR = 1;
    uint256 public constant SLASHING_BURN_FRACTION_DENOMINATOR = 1;

    /// @notice When an order is fulfilled, the market takes a fee based on the price of the order.
    /// This fraction is multiplied by the price to decide the fee.
    /// @dev The fee is configured as a constant to avoid accessing storage and thus paying for the
    /// gas of an SLOAD. This means the fee can only be changed by an implementation upgrade.
    /// Note that it is currently set to zero.
    uint256 public constant MARKET_FEE_NUMERATOR = 0;
    uint256 public constant MARKET_FEE_DENOMINATOR = 1;

    /// @notice Balance owned by the market contract itself. This balance is collected from fees,
    /// when the fee rate is set to a non-zero value.
    uint256 internal marketBalance;

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor(IRiscZeroVerifier verifier, bytes32 assessorId, address stakeTokenContract) {
        VERIFIER = verifier;
        ASSESSOR_ID = assessorId;
        STAKE_TOKEN_CONTRACT = stakeTokenContract;

        _disableInitializers();
    }

    function initialize(address initialOwner, string calldata _imageUrl) external initializer {
        __Ownable_init(initialOwner);
        __UUPSUpgradeable_init();
        __EIP712_init(BoundlessMarketLib.EIP712_DOMAIN, BoundlessMarketLib.EIP712_DOMAIN_VERSION);
        imageUrl = _imageUrl;
    }

    function setImageUrl(string calldata _imageUrl) external onlyOwner {
        imageUrl = _imageUrl;
    }

    function _authorizeUpgrade(address newImplementation) internal override onlyOwner {}

    // NOTE: We could verify the client signature here, but this adds about 18k gas (with a naive
    // implementation), doubling the cost of calling this method. It is not required for protocol
    // safety as the signature is checked during lockin, and during fulfillment (by the assessor).
    function submitRequest(ProofRequest calldata request, bytes calldata clientSignature) external payable {
        if (msg.value > 0) {
            deposit();
        }
        emit RequestSubmitted(request.id, request, clientSignature);
    }

    /// @inheritdoc IBoundlessMarket
    function lockRequest(ProofRequest calldata request, bytes calldata clientSignature) external {
        (address client, uint32 idx) = request.id.clientAndIndex();
        bytes32 requestDigest =
            request.verifyClientSignature(_hashTypedDataV4(request.eip712Digest()), client, clientSignature);
        uint64 deadline = request.validateRequest(accounts, client, idx);

        _lockRequest(request, requestDigest, client, idx, msg.sender, deadline);
    }

    /// @inheritdoc IBoundlessMarket
    function lockRequestWithSignature(
        ProofRequest calldata request,
        bytes calldata clientSignature,
        bytes calldata proverSignature
    ) external {
        (address client, uint32 idx) = request.id.clientAndIndex();
        bytes32 requestHash = _hashTypedDataV4(request.eip712Digest());
        bytes32 requestDigest = request.verifyClientSignature(requestHash, client, clientSignature);
        address prover = request.extractProverSignature(requestHash, proverSignature);
        uint64 deadline = request.validateRequest(accounts, client, idx);

        _lockRequest(request, requestDigest, client, idx, prover, deadline);
    }

    /// @notice Locks the request to the prover. Deducts funds from the client for payment
    /// and funding from the prover for locking stake.
    function _lockRequest(
        ProofRequest calldata request,
        bytes32 requestDigest,
        address client,
        uint32 idx,
        address prover,
        uint64 deadline
    ) internal {
        // Compute the current price offered by the reverse Dutch auction.
        uint96 price = request.offer.priceAtBlock(uint64(block.number)).toUint96();

        // Deduct funds from the client account and prover HP account.
        Account storage clientAccount = accounts[client];
        if (clientAccount.balance < price) {
            revert InsufficientBalance(client);
        }
        Account storage proverAccount = accounts[prover];
        if (proverAccount.isFrozen()) {
            revert AccountFrozen(prover);
        }
        if (proverAccount.stakeBalance < request.offer.lockStake.toUint96()) {
            revert InsufficientBalance(prover);
        }

        unchecked {
            clientAccount.balance -= price;
            proverAccount.stakeBalance -= request.offer.lockStake.toUint96();
        }

        // Record the lock for the request and emit an event.
        requestLocks[request.id] = RequestLock({
            prover: prover,
            price: price,
            deadline: deadline,
            stake: request.offer.lockStake.toUint96(),
            fingerprint: bytes8(requestDigest)
        });

        clientAccount.setRequestLocked(idx);
        emit RequestLocked(request.id, prover);
    }

    /// Validates the request and records the price to transient storage such that it can be
    /// fulfilled within the same transaction without taking a lock on it.
    function priceRequest(ProofRequest calldata request, bytes calldata clientSignature) public {
        (address client, uint32 idx) = request.id.clientAndIndex();
        bytes32 requestHash = _hashTypedDataV4(request.eip712Digest());
        bytes32 requestDigest = request.verifyClientSignature(requestHash, client, clientSignature);

        request.validateRequest(accounts, client, idx);

        // Compute the current price offered by the reverse Dutch auction.
        uint96 price = request.offer.priceAtBlock(uint64(block.number)).toUint96();

        // Record the price in transient storage, such that the order can be filled in this same transaction.
        TransientPrice({valid: true, price: price}).store(requestDigest);
    }

    /// @inheritdoc IBoundlessMarket
    function verifyDelivery(Fulfillment calldata fill, FulfillmentAssessor calldata assessorFill) public view {
        // Verify the application guest proof. We need to verify it here, even though the assessor
        // already verified that the prover has knowledge of a verifying receipt, because we need to
        // make sure the _delivered_ seal is valid.
        bytes32 claimDigest = ReceiptClaimLib.ok(fill.imageId, sha256(fill.journal)).digest();
        VERIFIER.verifyIntegrity{gas: FULFILL_MAX_GAS_FOR_VERIFY}(Receipt(fill.seal, claimDigest));

        // Verify the assessor, which ensures the application proof fulfills a valid request with the given ID.
        // NOTE: Signature checks and recursive verification happen inside the assessor.
        bytes32[] memory requestDigests = new bytes32[](1);
        requestDigests[0] = fill.requestDigest;
        bytes32 assessorJournalDigest = sha256(
            abi.encode(
                AssessorJournal({
                    requestDigests: requestDigests,
                    selectors: assessorFill.selectors,
                    root: claimDigest,
                    prover: assessorFill.prover
                })
            )
        );
        // Verification that the provided seal matches the required selector.
        if (assessorFill.selectors.indices.length == 1 && assessorFill.selectors.values[0] != bytes4(fill.seal[0:4])) {
            revert SelectorMismatch(assessorFill.selectors.values[0], bytes4(fill.seal[0:4]));
        }
        // Verification of the assessor seal does not need to comply with FULFILL_MAX_GAS_FOR_VERIFY.
        VERIFIER.verify(assessorFill.seal, ASSESSOR_ID, assessorJournalDigest);
    }

    /// @inheritdoc IBoundlessMarket
    function verifyBatchDelivery(Fulfillment[] calldata fills, FulfillmentAssessor calldata assessorFill) public view {
        // TODO(#242): Figure out how much the memory here is costing. If it's significant, we can do some tricks to reduce memory pressure.
        bytes32[] memory claimDigests = new bytes32[](fills.length);
        bytes32[] memory requestDigests = new bytes32[](fills.length);
        for (uint8 i = 0; i < fills.length; i++) {
            requestDigests[i] = fills[i].requestDigest;
            claimDigests[i] = ReceiptClaimLib.ok(fills[i].imageId, sha256(fills[i].journal)).digest();
            // Verification that the provided seal matches the required selector.
            for (uint8 j = 0; j < assessorFill.selectors.indices.length; j++) {
                if (assessorFill.selectors.indices[j] == i) {
                    if (assessorFill.selectors.values[j] != bytes4(fills[i].seal[0:4])) {
                        revert SelectorMismatch(assessorFill.selectors.values[j], bytes4(fills[i].seal[0:4]));
                    }
                    break;
                }
            }
            VERIFIER.verifyIntegrity{gas: FULFILL_MAX_GAS_FOR_VERIFY}(Receipt(fills[i].seal, claimDigests[i]));
        }
        bytes32 batchRoot = MerkleProofish.processTree(claimDigests);

        // Verify the assessor, which ensures the application proof fulfills a valid request with the given ID.
        // NOTE: Signature checks and recursive verification happen inside the assessor.
        bytes32 assessorJournalDigest = sha256(
            abi.encode(
                AssessorJournal({
                    requestDigests: requestDigests,
                    root: batchRoot,
                    selectors: assessorFill.selectors,
                    prover: assessorFill.prover
                })
            )
        );
        // Verification of the assessor seal does not need to comply with FULFILL_MAX_GAS_FOR_VERIFY.
        VERIFIER.verify(assessorFill.seal, ASSESSOR_ID, assessorJournalDigest);
    }

    /// @inheritdoc IBoundlessMarket
    function fulfill(Fulfillment calldata fill, FulfillmentAssessor calldata assessorFill) external {
        verifyDelivery(fill, assessorFill);
        _fulfillVerified(fill.id, fill.requestDigest, assessorFill.prover, fill.requirePayment);

        emit ProofDelivered(fill.id, fill.journal, fill.seal);
    }

    /// @inheritdoc IBoundlessMarket
    function fulfillBatch(Fulfillment[] calldata fills, FulfillmentAssessor calldata assessorFill) public {
        verifyBatchDelivery(fills, assessorFill);

        // NOTE: It would be slightly more efficient to keep balances and request flags in memory until a single
        // batch update to storage. However, updating the same storage slot twice only costs 100 gas, so
        // this savings is marginal, and will be outweighed by complicated memory management if not careful.
        for (uint256 i = 0; i < fills.length; i++) {
            _fulfillVerified(fills[i].id, fills[i].requestDigest, assessorFill.prover, fills[i].requirePayment);

            emit ProofDelivered(fills[i].id, fills[i].journal, fills[i].seal);
        }
    }

    /// @inheritdoc IBoundlessMarket
    function priceAndFulfillBatch(
        ProofRequest[] calldata requests,
        bytes[] calldata clientSignatures,
        Fulfillment[] calldata fills,
        FulfillmentAssessor calldata assessorFill
    ) external {
        for (uint256 i = 0; i < requests.length; i++) {
            priceRequest(requests[i], clientSignatures[i]);
        }
        fulfillBatch(fills, assessorFill);
    }

    /// Complete the fulfillment logic after having verified the app and assessor receipts.
    function _fulfillVerified(RequestId id, bytes32 requestDigest, address assessorProver, bool requirePayment)
        internal
    {
        (address client, uint32 idx) = id.clientAndIndex();

        (bool locked, bool fulfilled) = accounts[client].requestFlags(idx);

        bytes memory paymentError;
        if (locked) {
            paymentError = _fulfillVerifiedLocked(id, client, idx, requestDigest, fulfilled, assessorProver);
        } else {
            paymentError = _fulfillVerifiedUnlocked(id, client, idx, requestDigest, fulfilled, assessorProver);
        }

        if (paymentError.length > 0) {
            if (requirePayment) {
                revertWith(paymentError);
            } else {
                emit PaymentRequirementsFailed(paymentError);
            }
        }
    }

    function _fulfillVerifiedLocked(
        RequestId id,
        address client,
        uint32 idx,
        bytes32 requestDigest,
        bool fulfilled,
        address assessorProver
    ) internal returns (bytes memory paymentError) {
        RequestLock memory lock = requestLocks[id];

        // Check pre-conditions for transferring payment.
        if (lock.prover == address(0)) {
            // NOTE: This check is not strictly needed, as the fact that the lock has already been
            // zeroed out means zero value can be transferred. It is provided for clarity.
            return abi.encodeWithSelector(RequestIsFulfilled.selector, RequestId.unwrap(id));
        }
        if (lock.fingerprint != bytes8(requestDigest)) {
            revert RequestLockFingerprintDoesNotMatch({
                requestId: id,
                provided: bytes8(requestDigest),
                locked: lock.fingerprint
            });
        }

        // Mark the request as fulfilled.
        // NOTE: A request can become fulfilled even if the following checks fail, which control
        // whether payment will be sent. A later transaction can come to transfer the payment.
        if (!fulfilled) {
            accounts[client].setRequestFulfilled(idx);
            emit RequestFulfilled(id);
        }

        if (lock.deadline < block.number) {
            return abi.encodeWithSelector(RequestIsExpired.selector, RequestId.unwrap(id), lock.deadline);
        }
        if (lock.prover != assessorProver) {
            return abi.encodeWithSelector(RequestIsLocked.selector, RequestId.unwrap(id));
        }

        // Zero-out the lock to indicate that payment has been delivered and get a bit of a refund on gas.
        requestLocks[id] = RequestLock(address(0), uint96(0), uint64(0), uint96(0), bytes8(0));

        uint96 valueToProver = lock.price;
        if (MARKET_FEE_NUMERATOR > 0) {
            uint256 fee = uint256(lock.price) * MARKET_FEE_NUMERATOR / MARKET_FEE_DENOMINATOR;
            valueToProver -= fee.toUint96();
            marketBalance += fee;
        }
        Account storage proverAccount = accounts[lock.prover];
        proverAccount.balance += valueToProver;
        proverAccount.stakeBalance += lock.stake;
    }

    function _fulfillVerifiedUnlocked(
        RequestId id,
        address client,
        uint32 idx,
        bytes32 requestDigest,
        bool fulfilled,
        address assessorProver
    ) internal returns (bytes memory paymentError) {
        // When not locked, the fulfilled flag _does_ indicate that payment has already been transferred.
        if (fulfilled) {
            return abi.encodeWithSelector(RequestIsFulfilled.selector, RequestId.unwrap(id));
        }

        TransientPrice memory tprice = TransientPriceLibrary.load(requestDigest);

        if (!tprice.valid) {
            return abi.encodeWithSelector(RequestIsNotPriced.selector, RequestId.unwrap(id));
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
        uint96 valueToProver = tprice.price;
        if (MARKET_FEE_NUMERATOR > 0) {
            uint256 fee = uint256(tprice.price) * MARKET_FEE_NUMERATOR / MARKET_FEE_DENOMINATOR;
            valueToProver -= fee.toUint96();
            marketBalance += fee;
        }
        accounts[assessorProver].balance += valueToProver;
    }

    /// @inheritdoc IBoundlessMarket
    function submitRoot(address setVerifierAddress, bytes32 root, bytes calldata seal) external {
        IRiscZeroSetVerifier(address(setVerifierAddress)).submitMerkleRoot(root, seal);
    }

    /// @inheritdoc IBoundlessMarket
    function submitRootAndFulfillBatch(
        address setVerifier,
        bytes32 root,
        bytes calldata seal,
        Fulfillment[] calldata fills,
        FulfillmentAssessor calldata assessorFill
    ) external {
        IRiscZeroSetVerifier(address(setVerifier)).submitMerkleRoot(root, seal);
        fulfillBatch(fills, assessorFill);
    }

    /// @inheritdoc IBoundlessMarket
    function slash(RequestId requestId) external {
        (address client, uint32 idx) = requestId.clientAndIndex();
        (bool locked,) = accounts[client].requestFlags(idx);

        // Ensure the request is locked, and fetch the lock into memory.
        if (!locked) {
            revert RequestIsNotLocked({requestId: requestId});
        }
        RequestLock memory lock = requestLocks[requestId];

        if (lock.deadline >= block.number) {
            revert RequestIsNotExpired({requestId: requestId, deadline: lock.deadline});
        }

        // If the lock was cleared, the request is already finalized, either by fulfillment or slashing.
        if (lock.deadline == 0 && lock.stake == 0 && lock.fingerprint == bytes8(0)) {
            if (lock.prover == address(0)) {
                revert RequestIsFulfilled({requestId: requestId});
            }
            revert RequestIsSlashed({requestId: requestId});
        }

        // Zero out deadline, stake and fingerprint in storage to indicate that the request has been slashed.
        RequestLock storage lockStorage = requestLocks[requestId];
        lockStorage.deadline = 0;
        lockStorage.stake = 0;
        lockStorage.fingerprint = bytes8(0);

        // Calculate the portion of stake that should be burned vs sent to the client.
        // NOTE: If the burn fraction is not properly set, this can overflow.
        // The maximum feasible stake multiplied by the numerator must be less than 2^256.
        uint256 burnValue = uint256(lock.stake) * SLASHING_BURN_FRACTION_NUMERATOR / SLASHING_BURN_FRACTION_DENOMINATOR;
        uint256 transferValue = uint256(lock.stake) - burnValue;

        // Return the price to the client, plus the transfer value. Then burn the burn value.
        accounts[client].balance += lock.price;
        accounts[client].stakeBalance += transferValue.toUint96();

        // Freeze the prover account.
        Account storage proverAccount = accounts[lock.prover];
        proverAccount.setFrozen();

        // Transfer tokens from market to address zero.
        ERC20Burnable(STAKE_TOKEN_CONTRACT).burn(burnValue);

        emit ProverSlashed(requestId, lock.prover, burnValue, transferValue);
    }

    /// @inheritdoc IBoundlessMarket
    function imageInfo() external view returns (bytes32, string memory) {
        return (ASSESSOR_ID, imageUrl);
    }

    /// @inheritdoc IBoundlessMarket
    function deposit() public payable {
        accounts[msg.sender].balance += msg.value.toUint96();
        emit Deposit(msg.sender, msg.value);
    }

    /// @inheritdoc IBoundlessMarket
    function withdraw(uint256 value) public {
        if (accounts[msg.sender].balance < value.toUint96()) {
            revert InsufficientBalance(msg.sender);
        }
        unchecked {
            accounts[msg.sender].balance -= value.toUint96();
        }
        (bool sent,) = msg.sender.call{value: value}("");
        if (!sent) {
            revert TransferFailed();
        }
        emit Withdrawal(msg.sender, value);
    }

    /// @inheritdoc IBoundlessMarket
    function balanceOf(address addr) public view returns (uint256) {
        return uint256(accounts[addr].balance);
    }

    /// @inheritdoc IBoundlessMarket
    function depositStake(uint256 value) external {
        // Transfer tokens from user to market
        _depositStake(msg.sender, value);
    }

    /// @inheritdoc IBoundlessMarket
    function depositStakeWithPermit(uint256 value, uint256 deadline, uint8 v, bytes32 r, bytes32 s) external {
        // Transfer tokens from user to market
        try IERC20Permit(STAKE_TOKEN_CONTRACT).permit(msg.sender, address(this), value, deadline, v, r, s) {} catch {}
        _depositStake(msg.sender, value);
    }

    function _depositStake(address from, uint256 value) internal {
        IERC20(STAKE_TOKEN_CONTRACT).safeTransferFrom(from, address(this), value);
        accounts[from].stakeBalance += value.toUint96();
        emit StakeDeposit(from, value);
    }

    /// @inheritdoc IBoundlessMarket
    function withdrawStake(uint256 value) public {
        if (accounts[msg.sender].stakeBalance < value.toUint96()) {
            revert InsufficientBalance(msg.sender);
        }
        unchecked {
            accounts[msg.sender].stakeBalance -= value.toUint96();
        }
        // Transfer tokens from market to user
        bool success = IERC20(STAKE_TOKEN_CONTRACT).transfer(msg.sender, value);
        if (!success) revert TransferFailed();

        emit StakeWithdrawal(msg.sender, value);
    }

    /// @inheritdoc IBoundlessMarket
    function balanceOfStake(address addr) public view returns (uint256) {
        return uint256(accounts[addr].stakeBalance);
    }

    /// @inheritdoc IBoundlessMarket
    function accountIsFrozen(address addr) external view returns (bool) {
        Account storage prover = accounts[addr];
        return prover.isFrozen();
    }

    /// @inheritdoc IBoundlessMarket
    function unfreezeAccount() public {
        Account storage prover = accounts[msg.sender];
        prover.unsetFrozen();
    }

    /// @inheritdoc IBoundlessMarket
    function requestIsFulfilled(RequestId id) external view returns (bool) {
        (address client, uint32 idx) = id.clientAndIndex();
        (, bool fulfilled) = accounts[client].requestFlags(idx);
        return fulfilled;
    }

    /// @inheritdoc IBoundlessMarket
    function requestIsLocked(RequestId id) external view returns (bool) {
        (address client, uint32 idx) = id.clientAndIndex();
        (bool locked,) = accounts[client].requestFlags(idx);
        return locked;
    }

    /// @inheritdoc IBoundlessMarket
    function requestIsSlashed(RequestId id) external view returns (bool) {
        id.clientAndIndex();

        RequestLock memory lock = requestLocks[id];
        // Note, a stake and fingerprint of zero can exist on a valid request, however a deadline of zero cannot as
        // the request would be immediately expired, and expired requests cannot be locked in.
        return lock.deadline == 0 && lock.stake == 0 && lock.fingerprint == bytes8(0) && lock.prover != address(0);
    }

    /// @inheritdoc IBoundlessMarket
    function requestDeadline(RequestId id) external view returns (uint64) {
        return requestLocks[id].deadline;
    }

    /// @inheritdoc IBoundlessMarket
    function eip712DomainSeparator() external view returns (bytes32) {
        return _domainSeparatorV4();
    }

    /// Internal utility function to revert with a pre-encoded error.
    function revertWith(bytes memory err) internal pure {
        assembly {
            revert(add(err, 0x20), mload(err))
        }
    }
}
