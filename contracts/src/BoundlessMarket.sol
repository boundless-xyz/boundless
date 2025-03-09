// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.24;

import {ECDSA} from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import {SafeCast} from "@openzeppelin/contracts/utils/math/SafeCast.sol";
import {EIP712Upgradeable} from "@openzeppelin/contracts-upgradeable/utils/cryptography/EIP712Upgradeable.sol";
import {Ownable2StepUpgradeable} from "@openzeppelin/contracts-upgradeable/access/Ownable2StepUpgradeable.sol";
import {UUPSUpgradeable} from "@openzeppelin/contracts-upgradeable/proxy/utils/UUPSUpgradeable.sol";
import {Initializable} from "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";
import {ERC20} from "solmate/tokens/ERC20.sol";
import {ERC20Burnable} from "@openzeppelin/contracts/token/ERC20/extensions/ERC20Burnable.sol";
import {SafeTransferLib} from "solmate/utils/SafeTransferLib.sol";
import {IERC1271} from "@openzeppelin/contracts/interfaces/IERC1271.sol";
import {IRiscZeroVerifier, Receipt, ReceiptClaim, ReceiptClaimLib} from "risc0/IRiscZeroVerifier.sol";
import {IRiscZeroSetVerifier} from "risc0/IRiscZeroSetVerifier.sol";

import {IBoundlessMarket} from "./IBoundlessMarket.sol";
import {IBoundlessMarketCallback} from "./IBoundlessMarketCallback.sol";
import {Account} from "./types/Account.sol";
import {AssessorJournal} from "./types/AssessorJournal.sol";
import {AssessorCallback} from "./types/AssessorCallback.sol";
import {Fulfillment} from "./types/Fulfillment.sol";
import {AssessorReceipt} from "./types/AssessorReceipt.sol";
import {ProofRequest} from "./types/ProofRequest.sol";
import {RequestId} from "./types/RequestId.sol";
import {RequestLock} from "./types/RequestLock.sol";
import {FulfillmentContext, FulfillmentContextLibrary} from "./types/FulfillmentContext.sol";

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
    using SafeCast for int256;
    using SafeCast for uint256;
    using SafeTransferLib for ERC20;

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
    // safety as the signature is checked during lock, and during fulfillment (by the assessor).
    function submitRequest(ProofRequest calldata request, bytes calldata clientSignature) external payable {
        if (msg.value > 0) {
            deposit();
        }
        // No-op usage to avoid unused parameter warning.
        clientSignature;
        emit RequestSubmitted(request.id);
    }

    /// @inheritdoc IBoundlessMarket
    function lockRequest(ProofRequest calldata request, bytes calldata clientSignature) external {
        (address client, uint32 idx) = request.id.clientAndIndex();
        bytes32 requestHash = _verifyClientSignature(request, client, clientSignature);
        (uint64 lockDeadline, uint64 deadline) = request.validate();

        _lockRequest(request, requestHash, client, idx, msg.sender, lockDeadline, deadline);
    }

    /// @inheritdoc IBoundlessMarket
    function lockRequestWithSignature(
        ProofRequest calldata request,
        bytes calldata clientSignature,
        bytes calldata proverSignature
    ) external {
        (address client, uint32 idx) = request.id.clientAndIndex();
        bytes32 requestHash = _verifyClientSignature(request, client, clientSignature);
        address prover = _extractProverAddress(requestHash, proverSignature);
        (uint64 lockDeadline, uint64 deadline) = request.validate();

        _lockRequest(request, requestHash, client, idx, prover, lockDeadline, deadline);
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
        if (block.timestamp > lockDeadline) {
            revert RequestLockIsExpired({requestId: request.id, lockDeadline: lockDeadline});
        }

        // Compute the current price offered by the reverse Dutch auction.
        uint96 price = request.offer.priceAt(uint64(block.timestamp)).toUint96();

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
            requestDigest: requestDigest
        });

        clientAccount.setRequestLocked(idx);
        emit RequestLocked(request.id, prover);
    }

    /// Validates the request and records the price to transient storage such that it can be
    /// fulfilled within the same transaction without taking a lock on it.
    /// @inheritdoc IBoundlessMarket
    function priceRequest(ProofRequest calldata request, bytes calldata clientSignature) public {
        (address client, bool smartContractSigned) = request.id.clientAndIsSmartContractSigned();

        bytes32 requestHash;
        // We only need to validate the signature if it is a smart contract signature. This is because
        // EOA signatures are validated in the assessor during fulfillment, so the assessor guarantees
        // that the digest that is priced is one that was signed by the client.
        if (smartContractSigned) {
            requestHash = _verifyClientSignature(request, client, clientSignature);
        } else {
            requestHash = _hashTypedDataV4(request.eip712Digest());
        }

        request.validate();

        // Compute the current price offered by the reverse Dutch auction.
        uint96 price = request.offer.priceAt(uint64(block.timestamp)).toUint96();

        // Record the price in transient storage, such that the order can be filled in this same transaction.
        FulfillmentContext({valid: true, price: price}).store(requestHash);
    }

    /// @inheritdoc IBoundlessMarket
    function verifyDelivery(Fulfillment calldata fill, AssessorReceipt calldata assessorReceipt) public view {
        // Verify the application guest proof. We need to verify it here, even though the assessor
        // already verified that the prover has knowledge of a verifying receipt, because we need to
        // make sure the _delivered_ seal is valid.
        bytes32 claimDigest = ReceiptClaimLib.ok(fill.imageId, sha256(fill.journal)).digest();
        VERIFIER.verifyIntegrity{gas: FULFILL_MAX_GAS_FOR_VERIFY}(Receipt(fill.seal, claimDigest));

        // Verify the assessor, which ensures the application proof fulfills a valid request with the given ID.
        // Recursive verification happens inside the assessor.
        // NOTE: When signature checks are performed depends on whether the signature is a smart contract signature
        // or a regular EOA signature. It also depends on whether the request is locked or not.
        // Smart contract signatures are validated on-chain only, specifically when a request is locked, or when a request is priced.
        // EOA signatures are validated in the assessor during fulfillment. This design removes the need for EOA signatures to be
        // validated on-chain in any scenario at fulfillment time.
        bytes32[] memory requestDigests = new bytes32[](1);
        requestDigests[0] = fill.requestDigest;
        bytes32 assessorJournalDigest = sha256(
            abi.encode(
                AssessorJournal({
                    requestDigests: requestDigests,
                    selectors: assessorReceipt.selectors,
                    callbacks: assessorReceipt.callbacks,
                    root: claimDigest,
                    prover: assessorReceipt.prover
                })
            )
        );
        // Verification that the provided seal matches the required selector.
        // NOTE: Assessor guest ensures that the number of selectors <= the number of request digests in the journal.
        if (assessorReceipt.selectors.length > 0 && assessorReceipt.selectors[0].value != bytes4(fill.seal[0:4])) {
            revert SelectorMismatch(assessorReceipt.selectors[0].value, bytes4(fill.seal[0:4]));
        }
        // Verification of the assessor seal does not need to comply with FULFILL_MAX_GAS_FOR_VERIFY.
        VERIFIER.verify(assessorReceipt.seal, ASSESSOR_ID, assessorJournalDigest);
    }

    /// @inheritdoc IBoundlessMarket
    function verifyBatchDelivery(Fulfillment[] calldata fills, AssessorReceipt calldata assessorReceipt) public view {
        // TODO(#242): Figure out how much the memory here is costing. If it's significant, we can do some tricks to reduce memory pressure.
        // We can't handle more than 65535 fills in a single batch.
        // This is a limitation of the current Selector implementation,
        // that uses a uint16 for the index, and can be increased in the future.
        if (fills.length > type(uint16).max) {
            revert BatchSizeExceedsLimit(fills.length, type(uint16).max);
        }
        bytes32[] memory claimDigests = new bytes32[](fills.length);
        bytes32[] memory requestDigests = new bytes32[](fills.length);

        // Check the selector constraints.
        // NOTE: The assessor guest adds non-zero selector values to the list.
        uint256 selectorsLength = assessorReceipt.selectors.length;
        for (uint256 i = 0; i < selectorsLength; i++) {
            bytes4 expected = assessorReceipt.selectors[i].value;
            bytes4 received = bytes4(fills[assessorReceipt.selectors[i].index].seal[0:4]);
            if (expected != received) {
                revert SelectorMismatch(expected, received);
            }
        }

        // Verify the application receipts.
        for (uint256 i = 0; i < fills.length; i++) {
            Fulfillment calldata fill = fills[i];

            requestDigests[i] = fill.requestDigest;
            claimDigests[i] = ReceiptClaimLib.ok(fill.imageId, sha256(fill.journal)).digest();

            VERIFIER.verifyIntegrity{gas: FULFILL_MAX_GAS_FOR_VERIFY}(Receipt(fill.seal, claimDigests[i]));
        }

        bytes32 batchRoot = MerkleProofish.processTree(claimDigests);

        // Verify the assessor, which ensures the application proof fulfills a valid request with the given ID.
        // NOTE: Signature checks and recursive verification happen inside the assessor.
        bytes32 assessorJournalDigest = sha256(
            abi.encode(
                AssessorJournal({
                    requestDigests: requestDigests,
                    root: batchRoot,
                    callbacks: assessorReceipt.callbacks,
                    selectors: assessorReceipt.selectors,
                    prover: assessorReceipt.prover
                })
            )
        );
        // Verification of the assessor seal does not need to comply with FULFILL_MAX_GAS_FOR_VERIFY.
        VERIFIER.verify(assessorReceipt.seal, ASSESSOR_ID, assessorJournalDigest);
    }

    /// @inheritdoc IBoundlessMarket
    function priceAndFulfill(
        ProofRequest calldata request,
        bytes calldata clientSignature,
        Fulfillment calldata fill,
        AssessorReceipt calldata assessorReceipt
    ) external returns (bytes memory paymentError) {
        priceRequest(request, clientSignature);
        paymentError = fulfill(fill, assessorReceipt);
    }

    /// @inheritdoc IBoundlessMarket
    function priceAndFulfillBatch(
        ProofRequest[] calldata requests,
        bytes[] calldata clientSignatures,
        Fulfillment[] calldata fills,
        AssessorReceipt calldata assessorReceipt
    ) external returns (bytes[] memory paymentError) {
        for (uint256 i = 0; i < requests.length; i++) {
            priceRequest(requests[i], clientSignatures[i]);
        }
        paymentError = fulfillBatch(fills, assessorReceipt);
    }

    /// @inheritdoc IBoundlessMarket
    function fulfill(Fulfillment calldata fill, AssessorReceipt calldata assessorReceipt)
        public
        returns (bytes memory paymentError)
    {
        verifyDelivery(fill, assessorReceipt);

        // Execute the callback with the associated fulfillment information.
        // Note that if any of the following fulfillment logic fails, the entire transaction will
        // revert including this callback.
        if (assessorReceipt.callbacks.length > 0) {
            AssessorCallback memory callback = assessorReceipt.callbacks[0];
            _executeCallback(fill.id, callback.addr, callback.gasLimit, fill.imageId, fill.journal, fill.seal);
        }

        paymentError = _fulfillAndPay(fill, assessorReceipt.prover);
        emit ProofDelivered(fill.id);
    }

    /// @inheritdoc IBoundlessMarket
    function fulfillBatch(Fulfillment[] calldata fills, AssessorReceipt calldata assessorReceipt)
        public
        returns (bytes[] memory paymentError)
    {
        verifyBatchDelivery(fills, assessorReceipt);

        // Execute the callback with the associated fulfillment information.
        // Note that if any of the following fulfillment logic fails, the entire transaction will
        // revert including these callbacks.
        uint256 callbacksLength = assessorReceipt.callbacks.length;
        for (uint256 i = 0; i < callbacksLength; i++) {
            AssessorCallback memory callback = assessorReceipt.callbacks[i];
            Fulfillment calldata fill = fills[callback.index];
            _executeCallback(fill.id, callback.addr, callback.gasLimit, fill.imageId, fill.journal, fill.seal);
        }

        paymentError = new bytes[](fills.length);

        // NOTE: It would be slightly more efficient to keep balances and request flags in memory until a single
        // batch update to storage. However, updating the same storage slot twice only costs 100 gas, so
        // this savings is marginal, and will be outweighed by complicated memory management if not careful.
        for (uint256 i = 0; i < fills.length; i++) {
            paymentError[i] = _fulfillAndPay(fills[i], assessorReceipt.prover);
            emit ProofDelivered(fills[i].id);
        }
    }

    /// @inheritdoc IBoundlessMarket
    function priceAndFulfillAndWithdraw(
        ProofRequest calldata request,
        bytes calldata clientSignature,
        Fulfillment calldata fill,
        AssessorReceipt calldata assessorReceipt
    ) external returns (bytes memory paymentError) {
        priceRequest(request, clientSignature);
        paymentError = fulfillAndWithdraw(fill, assessorReceipt);
    }

    /// @inheritdoc IBoundlessMarket
    function priceAndFulfillBatchAndWithdraw(
        ProofRequest[] calldata requests,
        bytes[] calldata clientSignatures,
        Fulfillment[] calldata fills,
        AssessorReceipt calldata assessorReceipt
    ) external returns (bytes[] memory paymentError) {
        for (uint256 i = 0; i < requests.length; i++) {
            priceRequest(requests[i], clientSignatures[i]);
        }
        paymentError = fulfillBatchAndWithdraw(fills, assessorReceipt);
    }

    /// @inheritdoc IBoundlessMarket
    function fulfillAndWithdraw(Fulfillment calldata fill, AssessorReceipt calldata assessorReceipt)
        public
        returns (bytes memory paymentError)
    {
        paymentError = fulfill(fill, assessorReceipt);

        // Withdraw any remaining balance from the prover account.
        uint256 balance = accounts[assessorReceipt.prover].balance;
        if (balance > 0) {
            _withdraw(assessorReceipt.prover, balance);
        }
    }

    /// @inheritdoc IBoundlessMarket
    function fulfillBatchAndWithdraw(Fulfillment[] calldata fills, AssessorReceipt calldata assessorReceipt)
        public
        returns (bytes[] memory paymentError)
    {
        paymentError = fulfillBatch(fills, assessorReceipt);

        // Withdraw any remaining balance from the prover account.
        uint256 balance = accounts[assessorReceipt.prover].balance;
        if (balance > 0) {
            _withdraw(assessorReceipt.prover, balance);
        }
    }

    /// Complete the fulfillment logic after having verified the app and assessor receipts.
    function _fulfillAndPay(Fulfillment calldata fill, address prover) internal returns (bytes memory paymentError) {
        RequestId id = fill.id;
        (address client, uint32 idx) = id.clientAndIndex();
        Account storage clientAccount = accounts[client];
        (bool locked, bool fulfilled) = clientAccount.requestFlags(idx);

        if (locked) {
            RequestLock memory lock = requestLocks[id];
            if (lock.lockDeadline >= block.timestamp) {
                paymentError = _fulfillAndPayLocked(lock, id, client, idx, fill.requestDigest, fulfilled, prover);
            } else {
                paymentError = _fulfillAndPayWasLocked(lock, id, client, idx, fill.requestDigest, fulfilled, prover);
            }
        } else {
            paymentError = _fulfillAndPayNeverLocked(id, client, idx, fill.requestDigest, fulfilled, prover);
        }

        if (paymentError.length > 0) {
            emit PaymentRequirementsFailed(paymentError);
        }
    }

    /// @notice For a request that is currently locked. Marks the request as fulfilled, and transfers payment if eligible.
    /// @dev It is possible for anyone to fulfill a request at any time while the request has not expired.
    /// If the request is currently locked, only the prover can fulfill it and receive payment
    function _fulfillAndPayLocked(
        RequestLock memory lock,
        RequestId id,
        address client,
        uint32 idx,
        bytes32 requestDigest,
        bool fulfilled,
        address assessorProver
    ) internal returns (bytes memory paymentError) {
        if (lock.isProverPaid()) {
            return abi.encodeWithSelector(RequestIsFulfilled.selector, RequestId.unwrap(id));
        }

        if (lock.requestDigest != requestDigest) {
            revert InvalidRequestFulfillment({requestId: id, provided: requestDigest, locked: lock.requestDigest});
        }

        if (!fulfilled) {
            accounts[client].setRequestFulfilled(idx);
            emit RequestFulfilled(id);
        }

        // At this point the request has been fulfilled. The remaining logic determines whether
        // payment should be sent and to whom.
        // While the request is locked, only the locker is eligible for payment.
        if (lock.prover != assessorProver) {
            return abi.encodeWithSelector(RequestIsLocked.selector, RequestId.unwrap(id));
        }
        requestLocks[id].setProverPaidBeforeLockDeadline();

        uint96 price = lock.price;
        if (MARKET_FEE_BPS > 0) {
            price = _applyMarketFee(price);
        }
        accounts[assessorProver].balance += price;
        accounts[assessorProver].stakeBalance += lock.stake;
    }

    /// @notice For a request that was locked, and now the lock has expired. Marks the request as fulfilled,
    /// and transfers payment if eligible.
    /// @dev It is possible for anyone to fulfill a request at any time while the request has not expired.
    /// If the request was locked, and now the lock has expired, and the request as a whole has not expired,
    /// anyone can fulfill it and receive payment.
    function _fulfillAndPayWasLocked(
        RequestLock memory lock,
        RequestId id,
        address client,
        uint32 idx,
        bytes32 requestDigest,
        bool fulfilled,
        address assessorProver
    ) internal returns (bytes memory paymentError) {
        if (lock.isProverPaid()) {
            return abi.encodeWithSelector(RequestIsFulfilled.selector, RequestId.unwrap(id));
        }

        if (!fulfilled) {
            accounts[client].setRequestFulfilled(idx);
            emit RequestFulfilled(id);
        }

        // If no fulfillment context was stored for this request digest (via priceRequest),
        // then payment cannot be processed. This check also serves as
        // 1/ an expiration check since fulfillment contexts cannot be created for expired requests.
        // 2/ a smart contract signature check, since signatures are validated when a request is priced.
        FulfillmentContext memory context = FulfillmentContextLibrary.load(requestDigest);
        if (!context.valid) {
            revert RequestIsExpiredOrNotPriced(id);
        }
        uint96 price = context.price;

        // Deduct any additionally owned funds from client account. The client was already charged
        // for the price at lock time once when the request was locked. We only need to charge any
        // additional price for the difference between the price of the fulfilled request, at the
        // current block, and the price of the locked request.
        //
        // Note that although they have they same ID, the locked request and the fulfilled request
        // could be different. If the request fulfilled is the same as the one locked, the
        // `context.price` will be zero and the entire fee on the lock will be returned to the client.
        Account storage clientAccount = accounts[client];

        // If the request has the same id, but is different to the request that was locked, the fulfillment
        // price could be either higher or lower than the price that was previously locked.
        // If the price is higher, we charge the client the difference.
        // If the price is lower, we refund the client the difference.
        uint96 lockPrice = lock.price;
        if (price > lockPrice) {
            uint96 clientOwes = price - lockPrice;
            if (clientAccount.balance < clientOwes) {
                return abi.encodeWithSelector(InsufficientBalance.selector, client);
            }
            unchecked {
                clientAccount.balance -= clientOwes;
            }
        } else {
            int256 delta = uint256(price).toInt256() - uint256(lockPrice).toInt256();
            uint96 clientOwed = (-delta).toUint256().toUint96();
            clientAccount.balance += clientOwed;
        }

        requestLocks[id].setProverPaidAfterLockDeadline(assessorProver);
        if (MARKET_FEE_BPS > 0) {
            price = _applyMarketFee(price);
        }
        accounts[assessorProver].balance += price;
    }

    /// @notice For a request that has never been locked. Marks the request as fulfilled, and transfers payment if eligible.
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
        // When never locked, the fulfilled flag _does_ indicate that payment has already been transferred,
        // so we return early here.
        if (fulfilled) {
            return abi.encodeWithSelector(RequestIsFulfilled.selector, RequestId.unwrap(id));
        }

        // If no fulfillment context was stored for this request digest (via priceRequest),
        // then payment cannot be processed. This check also serves as an expiration check since
        // fulfillment contexts cannot be created for expired requests.
        FulfillmentContext memory context = FulfillmentContextLibrary.load(requestDigest);
        if (!context.valid) {
            return abi.encodeWithSelector(RequestIsExpiredOrNotPriced.selector, RequestId.unwrap(id));
        }
        uint96 price = context.price;

        Account storage clientAccount = accounts[client];
        if (!fulfilled) {
            clientAccount.setRequestFulfilled(idx);
            emit RequestFulfilled(id);
        }

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
        accounts[assessorProver].balance += price;
    }

    function _applyMarketFee(uint96 proverPayment) internal returns (uint96) {
        uint96 fee = proverPayment * MARKET_FEE_BPS / 10000;
        accounts[address(this)].balance += fee;
        return proverPayment - fee;
    }

    /// @notice Execute the callback for a fulfilled request if one is specified
    /// @dev This function is called after payment is processed and handles any callback specified in the request
    /// @param id The ID of the request being fulfilled
    /// @param callbackAddr The address of the callback contract
    /// @param callbackGasLimit The gas limit to use for the callback
    /// @param imageId The ID of the RISC Zero guest image that produced the proof
    /// @param journal The output journal from the RISC Zero guest execution
    /// @param seal The cryptographic seal proving correct execution
    function _executeCallback(
        RequestId id,
        address callbackAddr,
        uint96 callbackGasLimit,
        bytes32 imageId,
        bytes calldata journal,
        bytes calldata seal
    ) internal {
        try IBoundlessMarketCallback(callbackAddr).handleProof{gas: callbackGasLimit}(imageId, journal, seal) {}
        catch (bytes memory err) {
            emit CallbackFailed(id, callbackAddr, err);
        }
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
        AssessorReceipt calldata assessorReceipt
    ) external returns (bytes[] memory paymentError) {
        IRiscZeroSetVerifier(address(setVerifier)).submitMerkleRoot(root, seal);
        paymentError = fulfillBatch(fills, assessorReceipt);
    }

    /// @inheritdoc IBoundlessMarket
    function submitRootAndFulfillBatchAndWithdraw(
        address setVerifier,
        bytes32 root,
        bytes calldata seal,
        Fulfillment[] calldata fills,
        AssessorReceipt calldata assessorReceipt
    ) external returns (bytes[] memory paymentError) {
        IRiscZeroSetVerifier(address(setVerifier)).submitMerkleRoot(root, seal);
        paymentError = fulfillBatchAndWithdraw(fills, assessorReceipt);
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
        if (block.timestamp <= lock.deadline()) {
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

    function _withdraw(address account, uint256 value) internal {
        if (accounts[account].balance < value.toUint96()) {
            revert InsufficientBalance(account);
        }
        unchecked {
            accounts[account].balance -= value.toUint96();
        }
        (bool sent,) = account.call{value: value}("");
        if (!sent) {
            revert TransferFailed();
        }
        emit Withdrawal(account, value);
    }

    /// @inheritdoc IBoundlessMarket
    function withdraw(uint256 value) public {
        _withdraw(msg.sender, value);
    }

    /// @inheritdoc IBoundlessMarket
    function balanceOf(address addr) public view returns (uint256) {
        return uint256(accounts[addr].balance);
    }

    /// @inheritdoc IBoundlessMarket
    function withdrawFromTreasury(uint256 value) public onlyOwner {
        if (accounts[address(this)].balance < value.toUint96()) {
            revert InsufficientBalance(address(this));
        }
        unchecked {
            accounts[address(this)].balance -= value.toUint96();
        }
        (bool sent,) = msg.sender.call{value: value}("");
        if (!sent) {
            revert TransferFailed();
        }
        emit Withdrawal(address(this), value);
    }

    /// @inheritdoc IBoundlessMarket
    function depositStake(uint256 value) external {
        // Transfer tokens from user to market
        _depositStake(msg.sender, value);
    }

    /// @inheritdoc IBoundlessMarket
    function depositStakeWithPermit(uint256 value, uint256 deadline, uint8 v, bytes32 r, bytes32 s) external {
        // Transfer tokens from user to market
        try ERC20(STAKE_TOKEN_CONTRACT).permit(msg.sender, address(this), value, deadline, v, r, s) {} catch {}
        _depositStake(msg.sender, value);
    }

    function _depositStake(address from, uint256 value) internal {
        ERC20(STAKE_TOKEN_CONTRACT).safeTransferFrom(from, address(this), value);
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
        bool success = ERC20(STAKE_TOKEN_CONTRACT).transfer(msg.sender, value);
        if (!success) revert TransferFailed();

        emit StakeWithdrawal(msg.sender, value);
    }

    /// @inheritdoc IBoundlessMarket
    function balanceOfStake(address addr) public view returns (uint256) {
        return uint256(accounts[addr].stakeBalance);
    }

    /// @inheritdoc IBoundlessMarket
    function withdrawFromStakeTreasury(uint256 value) public onlyOwner {
        if (accounts[address(this)].stakeBalance < value.toUint96()) {
            revert InsufficientBalance(address(this));
        }
        unchecked {
            accounts[address(this)].stakeBalance -= value.toUint96();
        }
        bool success = ERC20(STAKE_TOKEN_CONTRACT).transfer(msg.sender, value);
        if (!success) revert TransferFailed();

        emit StakeWithdrawal(address(this), value);
    }

    /// @inheritdoc IBoundlessMarket
    function requestIsFulfilled(RequestId id) public view returns (bool) {
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

    function _verifyClientSignature(ProofRequest calldata request, address addr, bytes calldata clientSignature)
        internal
        view
        returns (bytes32)
    {
        bytes32 requestHash = _hashTypedDataV4(request.eip712Digest());
        if (request.id.isSmartContractSigned()) {
            if (IERC1271(addr).isValidSignature(requestHash, clientSignature) != IERC1271.isValidSignature.selector) {
                revert IBoundlessMarket.InvalidSignature();
            }
        } else {
            if (ECDSA.recover(requestHash, clientSignature) != addr) {
                revert IBoundlessMarket.InvalidSignature();
            }
        }
        return requestHash;
    }

    function _extractProverAddress(bytes32 requestHash, bytes calldata proverSignature)
        internal
        pure
        returns (address)
    {
        return ECDSA.recover(requestHash, proverSignature);
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
