// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.24;

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
import {ProofRequest} from "./types/ProofRequest.sol";
import {RequestId} from "./types/RequestId.sol";
import {RequestLock} from "./types/RequestLock.sol";
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

    /// Mapping of request ID to lock-in state. Non-zero for requests that are locked in.
    mapping(RequestId => RequestLock) public requestLocks;
    /// Mapping of address to account state.
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
    /// is burned, and the remaining portion is either send to the prover that ultimately fulfilled
    /// the order, or to the market treasury. This fraction controls that ratio.
    /// @dev The fee is configured as a constant to avoid accessing storage and thus paying for the
    /// gas of an SLOAD. Can only be changed via contract upgrade.
    uint256 public constant SLASHING_BURN_BPS = 7500;

    /// @notice When an order is fulfilled, the market takes a fee based on the price of the order.
    /// This fraction is multiplied by the price to decide the fee.
    /// @dev The fee is configured as a constant to avoid accessing storage and thus paying for the
    /// gas of an SLOAD. Can only be changed via contract upgrade.
    uint96 public constant MARKET_FEE_BPS = 0;

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
        (uint64 lockDeadline, uint64 deadline) = request.validateForLockRequest(accounts, client, idx);

        _lockRequest(request, requestDigest, client, idx, msg.sender, lockDeadline, deadline);
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
        (uint64 lockDeadline, uint64 deadline) = request.validateForLockRequest(accounts, client, idx);

        _lockRequest(request, requestDigest, client, idx, prover, lockDeadline, deadline);
    }

    /// @notice Locks the request to the prover. Deducts funds from the client for payment
    /// and funding from the prover for locking stake.
    function _lockRequest(
        ProofRequest calldata request,
        bytes32 requestDigest,
        address client,
        uint32 idx,
        address prover,
        uint64 lockDeadline,
        uint64 deadline
    ) internal {
        (bool locked, bool fulfilled) = accounts[client].requestFlags(idx);
        if (locked) {
            revert RequestIsLocked({requestId: request.id});
        }
        if (fulfilled) {
            revert RequestIsFulfilled({requestId: request.id});
        }

        // Compute the current price offered by the reverse Dutch auction.
        uint96 price = request.offer.priceAtBlock(uint64(block.number)).toUint96();

        // Deduct payment from the client account and stake from the prover account.
        Account storage clientAccount = accounts[client];
        if (clientAccount.balance < price) {
            revert InsufficientBalance(client);
        }
        Account storage proverAccount = accounts[prover];
        if (proverAccount.stakeBalance < request.offer.lockStake) {
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
            requestLockFlags: 0,
            lockDeadline: lockDeadline,
            deadlineDelta: uint256(deadline - lockDeadline).toUint24(),
            stake: request.offer.lockStake.toUint96(),
            fingerprint: bytes8(requestDigest)
        });

        clientAccount.setRequestLocked(idx);
        emit RequestLocked(request.id, prover);
    }

    /// Validates the request and records the price to transient storage such that it can be
    /// fulfilled within the same transaction without taking a lock on it.
    /// @inheritdoc IBoundlessMarket
    function priceRequest(ProofRequest calldata request, bytes calldata clientSignature) public {
        (address client,) = request.id.clientAndIndex();
        bytes32 requestHash = _hashTypedDataV4(request.eip712Digest());
        bytes32 requestDigest = request.verifyClientSignature(requestHash, client, clientSignature);

        request.validateForPriceRequest();

        // Compute the current price offered by the reverse Dutch auction.
        uint96 price = request.offer.priceAtBlock(uint64(block.number)).toUint96();

        // Record the price in transient storage, such that the order can be filled in this same transaction.
        TransientPrice({valid: true, price: price}).store(requestDigest);
    }

    /// @inheritdoc IBoundlessMarket
    function verifyDelivery(Fulfillment calldata fill, bytes calldata assessorSeal, address prover) public view {
        // Verify the application guest proof. We need to verify it here, even though the assessor
        // already verified that the prover has knowledge of a verifying receipt, because we need to
        // make sure the _delivered_ seal is valid.
        bytes32 claimDigest = ReceiptClaimLib.ok(fill.imageId, sha256(fill.journal)).digest();
        VERIFIER.verifyIntegrity{gas: FULFILL_MAX_GAS_FOR_VERIFY}(Receipt(fill.seal, claimDigest));

        // Verify the assessor, which ensures the application proof fulfills a valid request with the given ID.
        // NOTE: Signature checks and recursive verification happen inside the assessor.
        bytes32[] memory requestDigests = new bytes32[](1);
        requestDigests[0] = fill.requestDigest;
        bytes32 assessorJournalDigest =
            sha256(abi.encode(AssessorJournal({requestDigests: requestDigests, root: claimDigest, prover: prover})));
        // Verification of the assessor seal does not need to comply with FULFILL_MAX_GAS_FOR_VERIFY.
        VERIFIER.verify(assessorSeal, ASSESSOR_ID, assessorJournalDigest);
    }

    /// @inheritdoc IBoundlessMarket
    function verifyBatchDelivery(Fulfillment[] calldata fills, bytes calldata assessorSeal, address prover)
        public
        view
    {
        // TODO(#242): Figure out how much the memory here is costing. If it's significant, we can do some tricks to reduce memory pressure.
        bytes32[] memory claimDigests = new bytes32[](fills.length);
        bytes32[] memory requestDigests = new bytes32[](fills.length);
        for (uint256 i = 0; i < fills.length; i++) {
            requestDigests[i] = fills[i].requestDigest;
            claimDigests[i] = ReceiptClaimLib.ok(fills[i].imageId, sha256(fills[i].journal)).digest();
            VERIFIER.verifyIntegrity{gas: FULFILL_MAX_GAS_FOR_VERIFY}(Receipt(fills[i].seal, claimDigests[i]));
        }
        bytes32 batchRoot = MerkleProofish.processTree(claimDigests);

        // Verify the assessor, which ensures the application proof fulfills a valid request with the given ID.
        // NOTE: Signature checks and recursive verification happen inside the assessor.
        bytes32 assessorJournalDigest =
            sha256(abi.encode(AssessorJournal({requestDigests: requestDigests, root: batchRoot, prover: prover})));
        // Verification of the assessor seal does not need to comply with FULFILL_MAX_GAS_FOR_VERIFY.
        VERIFIER.verify(assessorSeal, ASSESSOR_ID, assessorJournalDigest);
    }

    /// @inheritdoc IBoundlessMarket
    function priceAndFulfill(
        ProofRequest calldata request,
        bytes calldata clientSignature,
        Fulfillment calldata fill,
        bytes calldata assessorSeal,
        address prover
    ) external {
        priceRequest(request, clientSignature);
        fulfill(fill, assessorSeal, prover);
    }

    /// @inheritdoc IBoundlessMarket
    function priceAndFulfillBatch(
        ProofRequest[] calldata requests,
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

    /// @inheritdoc IBoundlessMarket
    function fulfill(Fulfillment calldata fill, bytes calldata assessorSeal, address prover) public {
        verifyDelivery(fill, assessorSeal, prover);
        _fulfillAndPay(fill.id, fill.requestDigest, prover, fill.requirePayment);

        emit ProofDelivered(fill.id, fill.journal, fill.seal);
    }

    /// @inheritdoc IBoundlessMarket
    function fulfillBatch(Fulfillment[] calldata fills, bytes calldata assessorSeal, address prover) public {
        verifyBatchDelivery(fills, assessorSeal, prover);

        // NOTE: It would be slightly more efficient to keep balances and request flags in memory until a single
        // batch update to storage. However, updating the same storage slot twice only costs 100 gas, so
        // this savings is marginal, and will be outweighed by complicated memory management if not careful.
        for (uint256 i = 0; i < fills.length; i++) {
            _fulfillAndPay(fills[i].id, fills[i].requestDigest, prover, fills[i].requirePayment);

            emit ProofDelivered(fills[i].id, fills[i].journal, fills[i].seal);
        }
    }

    /// Complete the fulfillment logic after having verified the app and assessor receipts.
    function _fulfillAndPay(RequestId id, bytes32 requestDigest, address assessorProver, bool requirePayment)
        internal
    {
        (address client, uint32 idx) = id.clientAndIndex();
        (bool locked, bool fulfilled) = accounts[client].requestFlags(idx);

        bytes memory paymentError;
        if (locked) {
            paymentError = _fulfillAndPayLocked(id, client, idx, requestDigest, fulfilled, assessorProver);
        } else {
            paymentError = _fulfillAndPayNeverLocked(id, client, idx, requestDigest, fulfilled, assessorProver);
        }

        if (paymentError.length > 0) {
            if (requirePayment) {
                revertWith(paymentError);
            } else {
                emit PaymentRequirementsFailed(paymentError);
            }
        }
    }

    /// @notice For a request that has once been locked (could be locked now or the lock could have expired),
    /// mark the request as fulfilled and transfer payment if eligible.
    /// @dev It is possible for anyone to fulfill a request at any time while the request has not expired.
    /// Whether they will receive payment depends on the following conditions:
    /// - If the request is currently locked, only the prover can fulfill it and receive payment
    /// - If the request lock has now expired, but the request itself has not expired, anyone can fulfill
    ///   it and receive payment
    function _fulfillAndPayLocked(
        RequestId id,
        address client,
        uint32 idx,
        bytes32 requestDigest,
        bool fulfilled,
        address assessorProver
    ) internal returns (bytes memory paymentError) {
        RequestLock memory lock = requestLocks[id];
        if (lock.isProverPaid()) {
            return abi.encodeWithSelector(RequestIsFulfilled.selector, RequestId.unwrap(id));
        }

        if (lock.fingerprint != bytes8(requestDigest)) {
            revert RequestLockFingerprintDoesNotMatch({
                requestId: id,
                provided: bytes8(requestDigest),
                locked: lock.fingerprint
            });
        }

        if (!fulfilled) {
            accounts[client].setRequestFulfilled(idx);
            emit RequestFulfilled(id);
        }

        // At this point the request has been fulfilled. The remaining logic determines whether
        // payment should be sent and to whom.
        if (lock.lockDeadline >= block.number) {
            return _payLockedCurrently(id, assessorProver, lock.prover, lock.price, lock.stake);
        } else {
            return _payLockedExpired(id, assessorProver, client, lock.price, requestDigest);
        }
    }

    /// The request was locked, and the lock is still ongoing.
    /// Determines whether payment should be sent, and sends if so.
    function _payLockedCurrently(
        RequestId id,
        address assessorProver,
        address lockProver,
        uint96 price,
        uint96 lockStake
    ) internal returns (bytes memory paymentError) {
        // While the request is locked, only the locker is eligible for payment.
        if (lockProver != assessorProver) {
            return abi.encodeWithSelector(RequestIsLocked.selector, RequestId.unwrap(id));
        }
        requestLocks[id].setProverPaidBeforeLockDeadline();

        if (MARKET_FEE_BPS > 0) {
            price = _applyMarketFee(price);
        }
        accounts[assessorProver].balance += price;
        accounts[assessorProver].stakeBalance += lockStake;
    }

    /// The request was locked, the lock is now expired, but the request itself has not expired.
    /// Determines whether payment should be sent, and sends if so.
    function _payLockedExpired(
        RequestId id,
        address assessorProver,
        address client,
        uint96 lockPrice,
        bytes32 requestDigest
    ) internal returns (bytes memory paymentError) {
        // If the request was not priced in advance, no payment is sent.
        // It is not possible for an expired request to be priced, so this check doubles as a check
        // for whether the request has expired.
        TransientPrice memory tprice = TransientPriceLibrary.load(requestDigest);
        if (!tprice.valid) {
            return abi.encodeWithSelector(RequestIsNotPriced.selector, RequestId.unwrap(id));
        }
        uint96 price = tprice.price;

        // Deduct any additionally owned funds from client account. The client was already charged
        // for the price at lock time once when the request was locked. We only need to charge any
        // additional price increases from the dutch auction between lock time to now.
        Account storage clientAccount = accounts[client];
        uint96 clientOwes = price - lockPrice;
        if (clientAccount.balance < clientOwes) {
            return abi.encodeWithSelector(InsufficientBalance.selector, client);
        }
        requestLocks[id].setProverPaidAfterLockDeadline(assessorProver);

        unchecked {
            clientAccount.balance -= clientOwes;
        }
        if (MARKET_FEE_BPS > 0) {
            price = _applyMarketFee(price);
        }
        accounts[assessorProver].balance += price;
    }

    /// Fulfill a request that has never been locked.
    /// @dev If a never locked request is fulfilled, but fails the requirements for payment, no
    /// payment can ever be rendered for this order in the future.
    function _fulfillAndPayNeverLocked(
        RequestId id,
        address client,
        uint32 idx,
        bytes32 requestDigest,
        bool fulfilled,
        address assessorProver
    ) internal returns (bytes memory paymentError) {
        if (fulfilled) {
            return abi.encodeWithSelector(RequestIsFulfilled.selector, RequestId.unwrap(id));
        }

        // It is not possible for an expired request to be priced, so this check functions as a check
        // for whether the request has expired.
        TransientPrice memory tprice = TransientPriceLibrary.load(requestDigest);
        if (!tprice.valid) {
            return abi.encodeWithSelector(RequestIsNotPriced.selector, RequestId.unwrap(id));
        }

        Account storage clientAccount = accounts[client];
        if (!fulfilled) {
            clientAccount.setRequestFulfilled(idx);
            emit RequestFulfilled(id);
        }

        Account storage assessorProverAccount = accounts[assessorProver];
        _payNeverLocked(tprice.price, client, clientAccount, assessorProverAccount);
    }

    /// The request was never locked and was fulfilled in this transaction.
    /// Determines whether payment should be sent, and sends if so.
    function _payNeverLocked(
        uint96 price,
        address client,
        Account storage clientAccount,
        Account storage assessorProverAccount
    ) internal returns (bytes memory paymentError) {
        // Deduct the funds from client account.
        if (clientAccount.balance < price) {
            return abi.encodeWithSelector(InsufficientBalance.selector, client);
        }
        unchecked {
            clientAccount.balance -= price;
        }

        if (MARKET_FEE_BPS > 0) {
            price = _applyMarketFee(price);
        }
        assessorProverAccount.balance += price;
    }

    function _applyMarketFee(uint96 proverPayment) internal returns (uint96) {
        uint96 fee = proverPayment * MARKET_FEE_BPS / 10000;
        marketBalance += fee;
        return proverPayment - fee;
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
        bytes calldata assessorSeal,
        address prover
    ) external {
        IRiscZeroSetVerifier(address(setVerifier)).submitMerkleRoot(root, seal);
        fulfillBatch(fills, assessorSeal, prover);
    }

    /// @inheritdoc IBoundlessMarket
    function slash(RequestId requestId) external {
        (address client, uint32 idx) = requestId.clientAndIndex();
        (bool locked,) = accounts[client].requestFlags(idx);
        if (!locked) {
            revert RequestIsNotLocked({requestId: requestId});
        }

        RequestLock memory lock = requestLocks[requestId];
        if (lock.isSlashed()) {
            revert RequestIsSlashed({requestId: requestId});
        }
        if (lock.isProverPaidBeforeLockDeadline()) {
            revert RequestIsFulfilled({requestId: requestId});
        }

        // You can only slash a request after the request fully expires, so that if the request
        // does get fulfilled, we know which prover should receive a portion of the stake.
        if (block.number <= lock.deadline()) {
            revert RequestIsNotExpired({requestId: requestId, deadline: lock.deadline()});
        }

        // Request was either fulfilled after the lock deadline or the request expired unfulfilled.
        // In both cases the locker should be slashed.
        requestLocks[requestId].setSlashed();

        // Calculate the portion of stake that should be burned vs sent to the prover.
        uint256 burnValue = uint256(lock.stake) * SLASHING_BURN_BPS / 10000;
        ERC20Burnable(STAKE_TOKEN_CONTRACT).burn(burnValue);

        // If a prover fulfilled the request after the lock deadline, that prover
        // receives the unburned portion of the stake as a reward.
        // Otherwise the request expired unfulfilled, unburnt stake accrues to the market treasury,
        // and we refund the client the price they paid for the request at lock time.
        uint96 transferValue = (uint256(lock.stake) - burnValue).toUint96();
        address stakeRecipient = lock.prover;
        if (lock.isProverPaidAfterLockDeadline()) {
            // At this point lock.prover is the prover that ultimately fulfilled the request, not
            // the prover that locked the request. Transfer them the unburnt stake.
            accounts[lock.prover].stakeBalance += transferValue;
        } else {
            stakeRecipient = address(this);
            accounts[address(this)].stakeBalance += transferValue;
            accounts[client].balance += lock.price;
        }

        emit ProverSlashed(requestId, burnValue, transferValue, stakeRecipient);
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
    function requestIsFulfilled(RequestId id) external view returns (bool) {
        (address client, uint32 idx) = id.clientAndIndex();
        (, bool fulfilled) = accounts[client].requestFlags(idx);
        return fulfilled;
    }

    /// @inheritdoc IBoundlessMarket
    function requestIsLocked(RequestId id) public view returns (bool) {
        (address client, uint32 idx) = id.clientAndIndex();
        (bool locked,) = accounts[client].requestFlags(idx);
        return locked;
    }

    /// @inheritdoc IBoundlessMarket
    function requestIsSlashed(RequestId id) external view returns (bool) {
        return requestLocks[id].isSlashed();
    }

    /// @inheritdoc IBoundlessMarket
    function requestLockDeadline(RequestId id) external view returns (uint64) {
        if (!requestIsLocked(id)) {
            revert RequestIsNotLocked({requestId: id});
        }
        return requestLocks[id].lockDeadline;
    }

    /// @inheritdoc IBoundlessMarket
    function requestDeadline(RequestId id) external view returns (uint64) {
        if (!requestIsLocked(id)) {
            revert RequestIsNotLocked({requestId: id});
        }
        return requestLocks[id].deadline();
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
