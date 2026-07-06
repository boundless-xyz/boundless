// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
// SPDX-License-Identifier: BUSL-1.1

pragma solidity ^0.8.26;

import {ECDSA} from "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import {SafeCast} from "@openzeppelin/contracts/utils/math/SafeCast.sol";
import {EIP712Upgradeable} from "@openzeppelin/contracts-upgradeable/utils/cryptography/EIP712Upgradeable.sol";
import {AccessControlUpgradeable} from "@openzeppelin/contracts-upgradeable/access/AccessControlUpgradeable.sol";
import {UUPSUpgradeable} from "@openzeppelin/contracts-upgradeable/proxy/utils/UUPSUpgradeable.sol";
import {Initializable} from "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";
import {Proxy} from "@openzeppelin/contracts/proxy/Proxy.sol";
import {ERC20} from "solmate/tokens/ERC20.sol";
import {SafeTransferLib} from "solmate/utils/SafeTransferLib.sol";
import {IERC1271} from "@openzeppelin/contracts/interfaces/IERC1271.sol";
import {IRiscZeroSetVerifier} from "risc0/IRiscZeroSetVerifier.sol";

import {IBoundlessMarket} from "../../src/IBoundlessMarket.sol";
import {IBoundlessMarketCallback} from "../../src/IBoundlessMarketCallback.sol";
import {Account} from "../../src/types/Account.sol";
import {Fulfillment, LegacyFulfillment} from "../../src/types/Fulfillment.sol";
import {FulfillmentDataLibrary, FulfillmentDataType} from "../../src/types/FulfillmentData.sol";
import {ProofRequest} from "../../src/types/ProofRequest.sol";
import {LockRequestLibrary} from "../../src/types/LockRequest.sol";
import {RequestId} from "../../src/types/RequestId.sol";
import {RequestLock} from "../../src/types/RequestLock.sol";
import {ProofRequestBatch} from "../../src/types/ProofRequestBatch.sol";
import {SlimRequest, SlimRequestLibrary} from "../../src/types/SlimRequest.sol";
import {FulfillmentBatch} from "../../src/types/FulfillmentBatch.sol";
import {FulfillmentContext, FulfillmentContextLibrary} from "./FulfillmentContext.sol";

import {BoundlessMarketLib} from "../../src/libraries/BoundlessMarketLib.sol";

import {IBoundlessRouter} from "../../src/router/interfaces/IBoundlessRouter.sol";

error InvalidRouter();
error InvalidCollateralToken();
error InvalidLegacyImpl();
error InvalidInitialOwner();
error MismatchedRequestId(uint256 expected, uint256 received);

contract BoundlessMarket is
    IBoundlessMarket,
    Initializable,
    EIP712Upgradeable,
    AccessControlUpgradeable,
    UUPSUpgradeable,
    Proxy
{
    using SafeCast for int256;
    using SafeCast for uint256;
    using SafeTransferLib for ERC20;

    /// @dev The version of the contract, with respect to upgrades.
    uint64 public constant VERSION = 1;

    /// @notice Admin role identifier
    bytes32 public constant ADMIN_ROLE = DEFAULT_ADMIN_ROLE;

    /// Mapping of request ID to lock-in state. Non-zero for requests that are locked in.
    mapping(RequestId => RequestLock) public requestLocks;
    /// Mapping of address to account state.
    mapping(address => Account) internal accounts;
    /// @dev Held the assessor guest URL in earlier implementations. The market
    ///      no longer reads or writes this field; the slot is preserved so the
    ///      storage layout doesn't shift across upgrades. Kept under its
    ///      original name so the OZ storage-layout check accepts the upgrade
    ///      without a rename annotation.
    string private imageUrl;

    /// @notice Block number at which a fulfillment commitment was recorded, keyed by the commitment
    ///         hash `keccak256(abi.encode(FulfillmentBatch[]))`. Anti-front-running guard for the
    ///         open fulfillment paths (never-locked and after the lock deadline), which carry no
    ///         prior on-chain prover binding.
    mapping(bytes32 => uint256) public fulfillmentCommitBlock;

    /// @notice The verification engine. The market calls `ROUTER.verifyBatch`
    ///         once per fulfillment batch and trusts whatever per-class adapter the
    ///         router dispatches to.
    /// @dev    Set in the constructor; pinned per implementation contract.
    /// @custom:oz-upgrades-unsafe-allow state-variable-immutable
    IBoundlessRouter public immutable ROUTER;
    /// @custom:oz-upgrades-unsafe-allow state-variable-immutable
    address public immutable COLLATERAL_TOKEN_CONTRACT;

    /// @notice Implementation address of the previous (legacy ABI) BoundlessMarket.
    ///         The fallback function delegate-calls into this address so the
    ///         pre-router ABI keeps working for in-flight transactions and old
    ///         broker clients during the migration window.
    /// @dev    On Base mainnet this is the impl pointed to by the proxy before
    ///         the upgrade. On dev/localnet a fresh deployment of
    ///         contracts/src/legacy/BoundlessMarketLegacy.sol.
    /// @custom:oz-upgrades-unsafe-allow state-variable-immutable
    address public immutable LEGACY_IMPL;

    /// @notice Max gas allowed for ERC1271 smart contract signature checks used for client auth.
    /// @dev This constraint is applied to smart contract signatures used for authorizing proof
    /// requests in order to make gas costs bounded.
    uint256 public constant ERC1271_MAX_GAS_FOR_CHECK = 100000;

    /// @notice When a prover is slashed for failing to fulfill a request, a portion of the collateral
    /// is burned, and the remaining portion is either send to the prover that ultimately fulfilled
    /// the order, or to the market treasury. This fraction controls that ratio.
    /// @dev The value is configured as a constant to avoid accessing storage and thus paying for the
    /// gas of an SLOAD. Can only be changed via contract upgrade.
    uint256 public constant SLASHING_BURN_BPS = 5000;

    /// @notice When an order is fulfilled, the market takes a fee based on the price of the order.
    /// This fraction is multiplied by the price to decide the fee.
    /// @dev The fee is configured as a constant to avoid accessing storage and thus paying for the
    /// gas of an SLOAD. Can only be changed via contract upgrade.
    uint96 public constant MARKET_FEE_BPS = 0;

    /// @notice Minimum number of blocks between a fulfillment commitment and its reveal on the open
    /// fulfillment paths. A reveal at block R requires a commitment at block C with
    /// `C + COMMIT_REVEAL_MIN_BLOCKS <= R`, so a front-runner who only learns the seal at reveal time
    /// cannot have committed early enough to steal it.
    uint256 public constant COMMIT_REVEAL_MIN_BLOCKS = 1;

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor(IBoundlessRouter router, address collateralTokenContract, address legacyImpl) {
        if (address(router) == address(0)) revert InvalidRouter();
        if (collateralTokenContract == address(0)) revert InvalidCollateralToken();
        if (legacyImpl == address(0)) revert InvalidLegacyImpl();

        ROUTER = router;
        COLLATERAL_TOKEN_CONTRACT = collateralTokenContract;
        LEGACY_IMPL = legacyImpl;

        _disableInitializers();
    }

    /// @notice OpenZeppelin {Proxy} hook: returns the address that selectors not
    ///         declared on this contract are delegate-called into. The inherited
    ///         `fallback()` forwards to it, preserving the caller, value, and the
    ///         proxy's storage context.
    /// @dev    This is the LEGACY ABI implementation, NOT this contract's ERC1967
    ///         implementation (that lives in the proxy's storage slot). Keeping
    ///         the legacy ABI surface live this way avoids re-introducing the
    ///         legacy bodies into this implementation's bytecode.
    function _implementation() internal view override returns (address) {
        return LEGACY_IMPL;
    }

    function initialize(address initialOwner) external initializer {
        if (initialOwner == address(0)) {
            revert InvalidInitialOwner();
        }
        __AccessControl_init();
        __UUPSUpgradeable_init();
        __EIP712_init(BoundlessMarketLib.EIP712_DOMAIN, BoundlessMarketLib.EIP712_DOMAIN_VERSION);
        _grantRole(ADMIN_ROLE, initialOwner);
    }

    function _authorizeUpgrade(address newImplementation) internal override onlyRole(ADMIN_ROLE) {}

    // NOTE: We could verify the client signature here, but this adds about 18k gas (with a naive
    // implementation), doubling the cost of calling this method. It is not required for protocol
    // safety as the signature is checked during lock, and during fulfillment (by the assessor).
    function submitRequest(ProofRequest calldata request, bytes calldata clientSignature) external payable {
        if (msg.value > 0) {
            deposit();
        }
        emit RequestSubmitted(request.id, request, clientSignature);
    }

    /// @inheritdoc IBoundlessMarket
    function lockRequest(ProofRequest calldata request, bytes calldata clientSignature) external {
        (address client, uint32 idx) = request.id.clientAndIndex();
        (bytes32 requestHash,) = _verifyClientSignature(request, client, clientSignature);
        (uint64 lockDeadline, uint64 deadline) = request.validate();

        _lockRequest(request, clientSignature, requestHash, client, idx, msg.sender, lockDeadline, deadline);
    }

    /// @inheritdoc IBoundlessMarket
    function lockRequestWithSignature(
        ProofRequest calldata request,
        bytes calldata clientSignature,
        bytes calldata proverSignature
    ) external {
        (address client, uint32 idx) = request.id.clientAndIndex();
        (bytes32 requestHash, bytes32 proofRequestEip712Digest) =
            _verifyClientSignature(request, client, clientSignature);
        bytes32 lockRequestHash =
            _hashTypedDataV4(LockRequestLibrary.eip712DigestFromPrecomputedDigest(proofRequestEip712Digest));
        address prover = ECDSA.recover(lockRequestHash, proverSignature);
        (uint64 lockDeadline, uint64 deadline) = request.validate();

        _lockRequest(request, clientSignature, requestHash, client, idx, prover, lockDeadline, deadline);
    }

    /// @notice Locks the request to the prover. Deducts funds from the client for payment
    /// and funding from the prover for locking collateral.
    function _lockRequest(
        ProofRequest calldata request,
        bytes calldata clientSignature,
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

        // Deduct payment from the client account and collateral from the prover account.
        Account storage clientAccount = accounts[client];
        if (clientAccount.balance < price) {
            revert InsufficientBalance(client);
        }
        Account storage proverAccount = accounts[prover];
        if (proverAccount.collateralBalance < request.offer.lockCollateral) {
            revert InsufficientBalance(prover);
        }

        unchecked {
            clientAccount.balance -= price;
            proverAccount.collateralBalance -= request.offer.lockCollateral.toUint96();
        }

        // Record the lock for the request and emit an event.
        requestLocks[request.id] = RequestLock({
            prover: prover,
            price: price,
            requestLockFlags: 0,
            lockDeadline: lockDeadline,
            deadlineDelta: uint256(deadline - lockDeadline).toUint24(),
            collateral: request.offer.lockCollateral.toUint96(),
            requestDigest: requestDigest
        });

        clientAccount.setRequestLocked(idx);
        emit RequestLocked(request.id, prover, request, clientSignature);
    }

    /// Validates the request and records the price to transient storage such that it can be
    /// fulfilled within the same transaction without taking a lock on it.
    /// @inheritdoc IBoundlessMarket
    function priceRequest(ProofRequest calldata request, bytes calldata clientSignature) public {
        address client = request.id.client();

        (bytes32 requestHash,) = _verifyClientSignature(request, client, clientSignature);

        (, uint64 deadline) = request.validate();
        bool expired = deadline < block.timestamp;

        // Compute the current price offered by the reverse Dutch auction.
        uint96 price = request.offer.priceAt(uint64(block.timestamp)).toUint96();

        // Record the price in transient storage, such that the order can be filled in this same transaction.
        FulfillmentContext({valid: true, expired: expired, price: price}).store(requestHash);
    }

    /// @dev Assert that the supplied `requestDigest` matches either the stored
    ///      lock digest or a valid `FulfillmentContext` entry from
    ///      `priceRequest`. Once this passes, the slim payload that produced
    ///      `requestDigest` is bound to a client-signed request and downstream
    ///      consumers (router, assessor adapter, callback dispatch) can trust
    ///      its fields without re-verification.
    ///
    ///      `requestDigest` is the domain-bound digest (`_hashTypedDataV4` of
    ///      the EIP-712 struct hash produced by
    ///      `SlimRequestLibrary.reconstructRequestDigest`). `_lockRequest` and
    ///      `priceRequest` both write this same domain-bound value into
    ///      storage, so this function can compare without further hashing.
    function _verifyBinding(RequestId id, bytes32 requestDigest) internal view {
        if (requestLocks[id].requestDigest == requestDigest) {
            return;
        }
        if (FulfillmentContextLibrary.load(requestDigest).valid) {
            return;
        }
        revert RequestIsNotLockedOrPriced(id);
    }

    /// @dev Per-batch helper: reconstruct each request's domain-bound digest,
    ///      verify the binding, and collect into an array for the router and assessor.
    function _bindAndCollectDigests(SlimRequest[] calldata requests)
        internal
        view
        returns (bytes32[] memory requestDigests)
    {
        uint256 n = requests.length;
        requestDigests = new bytes32[](n);
        for (uint256 i = 0; i < n; i++) {
            bytes32 requestDigest = _hashTypedDataV4(SlimRequestLibrary.reconstructRequestDigest(requests[i]));
            _verifyBinding(requests[i].id, requestDigest);
            requestDigests[i] = requestDigest;
        }
    }

    /// @inheritdoc IBoundlessMarket
    function priceAndFulfill(
        ProofRequestBatch[] calldata requestBatches,
        FulfillmentBatch[] calldata fulfillmentBatches
    ) public returns (bytes[] memory paymentError) {
        _priceAll(requestBatches);
        paymentError = fulfill(fulfillmentBatches);
    }

    /// @inheritdoc IBoundlessMarket
    function fulfill(FulfillmentBatch[] calldata fulfillmentBatches) public returns (bytes[] memory paymentError) {
        // Anti-front-running: the open fulfillment paths (never-locked, or locked-but-past-deadline)
        // carry no prior on-chain prover binding, so a copied proof could be re-submitted under a
        // different prover. Require a `commitFulfillment` recorded in a strictly earlier block —
        // binding these exact batches (and their seals) — before settling any open-path fill. The
        // locked-before-deadline path is already bound by `lock.prover` and needs no commitment.
        if (_hasOpenPathFill(fulfillmentBatches)) {
            _consumeCommitment(fulfillmentBatches);
        }

        // Flatten payment-error output across fulfillment batches.
        uint256 totalFills = 0;
        for (uint256 j = 0; j < fulfillmentBatches.length; j++) {
            totalFills += fulfillmentBatches[j].fills.length;
        }
        paymentError = new bytes[](totalFills);

        uint256 outIdx = 0;
        for (uint256 j = 0; j < fulfillmentBatches.length; j++) {
            FulfillmentBatch calldata batch = fulfillmentBatches[j];
            uint256 n = batch.fills.length;
            if (n == 0) continue;
            if (n > type(uint16).max) revert BatchSizeExceedsLimit(n, type(uint16).max);
            if (batch.requests.length != n) revert BatchSizeExceedsLimit(batch.requests.length, n);

            // Bind every slim payload to a client-signed request (lock or
            // priced), then dispatch verifier + assessor through the router
            // and settle each fill.
            bytes32[] memory requestDigests = _bindAndCollectDigests(batch.requests);
            ROUTER.verifyBatch(batch, requestDigests);
            outIdx = _settleBatch(batch, requestDigests, paymentError, outIdx);
        }
    }

    /// @notice Commit, ahead of time, to the exact fulfillment you will reveal on an open path.
    /// @param commitment `keccak256(abi.encode(FulfillmentBatch[]))` — the exact `fulfillmentBatches`
    ///        argument of the upcoming `fulfill` / `priceAndFulfill` / `submitRoot…` call. Because it
    ///        binds the prover and every seal, it cannot be precomputed without already holding the
    ///        proof, and the reveal must land at least `COMMIT_REVEAL_MIN_BLOCKS` blocks later.
    /// @dev First writer per commitment wins the block stamp; re-commits are no-ops. The commitment
    ///      reveals nothing (just a hash), so front-running this call is pointless.
    function commitFulfillment(bytes32 commitment) external {
        if (fulfillmentCommitBlock[commitment] == 0) {
            fulfillmentCommitBlock[commitment] = block.number;
        }
    }

    /// @dev True if any fill in the call settles on an open path — never-locked, or locked but past
    ///      its lock deadline — i.e. a path with no prior on-chain prover binding.
    function _hasOpenPathFill(FulfillmentBatch[] calldata fulfillmentBatches) internal view returns (bool) {
        for (uint256 j = 0; j < fulfillmentBatches.length; j++) {
            SlimRequest[] calldata requests = fulfillmentBatches[j].requests;
            for (uint256 i = 0; i < requests.length; i++) {
                (address client, uint32 idx) = requests[i].id.clientAndIndex();
                (bool locked,) = accounts[client].requestFlags(idx);
                if (!locked) return true; // never-locked
                if (requestLocks[requests[i].id].lockDeadline < block.timestamp) return true; // was-locked
            }
        }
        return false;
    }

    /// @dev Consume the commitment for these exact batches, requiring it was recorded at least
    ///      `COMMIT_REVEAL_MIN_BLOCKS` blocks earlier. Reverts if absent or too recent. One-shot.
    function _consumeCommitment(FulfillmentBatch[] calldata fulfillmentBatches) internal {
        bytes32 commitment = keccak256(abi.encode(fulfillmentBatches));
        uint256 committedBlock = fulfillmentCommitBlock[commitment];
        if (committedBlock == 0 || committedBlock + COMMIT_REVEAL_MIN_BLOCKS > block.number) {
            revert MissingFulfillmentCommitment();
        }
        delete fulfillmentCommitBlock[commitment];
    }

    /// @dev Per-fill settle pass for one already-verified `FulfillmentBatch`.
    ///      Walks every fill, charges/credits accounts via `_fulfillAndPay`,
    ///      and dispatches callbacks. Returns the updated flat-output index so
    ///      `fulfill` can keep packing payment errors across batches.
    function _settleBatch(
        FulfillmentBatch calldata batch,
        bytes32[] memory requestDigests,
        bytes[] memory paymentError,
        uint256 outIdx
    ) internal returns (uint256) {
        address prover = batch.prover;
        uint256 n = batch.fills.length;
        for (uint256 i = 0; i < n; i++) {
            Fulfillment calldata fill = batch.fills[i];
            SlimRequest calldata slim = batch.requests[i];
            bool expired;
            (paymentError[outIdx], expired) = _fulfillAndPay(fill, slim.id, requestDigests[i], prover);

            if (!expired && slim.callback.addr != address(0)) {
                if (fill.fulfillmentDataType == FulfillmentDataType.ImageIdAndJournal) {
                    (bytes32 imageId, bytes calldata journal) =
                        FulfillmentDataLibrary.decodePackedImageIdAndJournal(fill.fulfillmentData);
                    _executeCallback(slim.id, slim.callback.addr, slim.callback.gasLimit, imageId, journal, fill.seal);
                } else {
                    revert UnfulfillableCallback();
                }
            }
            outIdx++;
        }
        return outIdx;
    }

    /// @inheritdoc IBoundlessMarket
    function priceAndFulfillAndWithdraw(
        ProofRequestBatch[] calldata requestBatches,
        FulfillmentBatch[] calldata fulfillmentBatches
    ) public returns (bytes[] memory paymentError) {
        _priceAll(requestBatches);
        paymentError = fulfillAndWithdraw(fulfillmentBatches);
    }

    /// @inheritdoc IBoundlessMarket
    function fulfillAndWithdraw(FulfillmentBatch[] calldata fulfillmentBatches)
        public
        returns (bytes[] memory paymentError)
    {
        paymentError = fulfill(fulfillmentBatches);

        // Withdraw any remaining balance from each fulfillment batch's prover.
        for (uint256 j = 0; j < fulfillmentBatches.length; j++) {
            address prover = fulfillmentBatches[j].prover;
            uint256 balance = accounts[prover].balance;
            if (balance > 0) {
                _withdraw(prover, balance);
            }
        }
    }

    /// @dev Price every request in every group. Each `ProofRequestBatch`
    ///      carries the requests and matching client signatures that need
    ///      pricing — typically only the un-locked entries. Verified client
    ///      signatures populate `FulfillmentContext` keyed by `requestHash`,
    ///      which the subsequent `fulfill` step looks up via the slim
    ///      payload's reconstructed digest.
    function _priceAll(ProofRequestBatch[] calldata requestBatches) internal {
        for (uint256 j = 0; j < requestBatches.length; j++) {
            ProofRequest[] calldata requests = requestBatches[j].requests;
            bytes[] calldata sigs = requestBatches[j].signatures;
            if (sigs.length != requests.length) {
                revert BatchSizeExceedsLimit(sigs.length, requests.length);
            }
            for (uint256 i = 0; i < requests.length; i++) {
                priceRequest(requests[i], sigs[i]);
            }
        }
    }

    /// Complete the fulfillment logic after having verified the app and assessor
    /// receipts. `requestDigest` is the verified EIP-712 digest reconstructed
    /// from the slim payload; the caller has already asserted it matches the
    /// stored binding via `_verifyBinding`. `id` comes from the trusted slim
    /// payload (positionally paired with `fill`).
    function _fulfillAndPay(Fulfillment calldata fill, RequestId id, bytes32 requestDigest, address prover)
        internal
        returns (bytes memory paymentError, bool expired)
    {
        (address client, uint32 idx) = id.clientAndIndex();
        Account storage clientAccount = accounts[client];
        (bool locked, bool fulfilled) = clientAccount.requestFlags(idx);

        // Fetch the lock and fulfillment information.
        // NOTE: The `lock` should only be used in code paths where locked is true.
        RequestLock memory lock;
        if (locked) {
            lock = requestLocks[id];
        }
        FulfillmentContext memory context = FulfillmentContextLibrary.load(requestDigest);
        // Release the priced context after reading it. The default profile uses transient storage,
        // which clears automatically at the end of the transaction; this Shanghai variant uses
        // persistent storage, so it must clear the slot explicitly to bound storage growth and to
        // preserve the same single-transaction price-then-fulfill semantics (a priced context must
        // not survive into a later transaction). `_verifyBinding` only reads via the non-clearing
        // `load`, so this is the single consume point.
        FulfillmentContextLibrary.clear(requestDigest);

        // First, check whether the request is known to be a valid signed request, and whether it is
        // expired. If the request cannot be authenticated, revert.
        //
        // In the expired case, we return early here. We do not emit the ProofDelivered event, and
        // we do not issue a callback. This makes interpretation of the ProofDelivered events
        // simpler, as they cannot be emitted for an expired request.
        if (context.valid) {
            // Request has been validated in priceRequest, check the reported expiration.
            if (context.expired) {
                paymentError = abi.encodeWithSelector(RequestIsExpired.selector, RequestId.unwrap(id));
                emit PaymentRequirementsFailed(paymentError);
                return (paymentError, true);
            }
        } else if (locked && lock.requestDigest == requestDigest) {
            // Request was validated in lockRequest, check whether the request is fully expired.
            if (lock.deadline() < block.timestamp) {
                paymentError = abi.encodeWithSelector(RequestIsExpired.selector, RequestId.unwrap(id));
                emit PaymentRequirementsFailed(paymentError);
                return (paymentError, true);
            }
        } else {
            // Request is not validated by either price or lock step. We cannot determine that the
            // request is authentic, so we revert.
            // NOTE: We could loosen this slightly, only reverting when the id indicates this is a
            // smart-contract authorized request. However, we'd need to handle the fact that we
            // don't have a FulfillmentContext on this code path.
            revert RequestIsNotLockedOrPriced(id);
        }

        // NOTE: Every code path past this point must ensure the `fulfilled` flag is set, or
        // revert. If this is not the case, then it will break the invariant that the first
        // delivered proof (e.g. the first time `ProofDelivered` fires and the first time the
        // callback is called) the fulfilled flag is set.
        if (locked) {
            if (lock.lockDeadline >= block.timestamp) {
                paymentError = _fulfillAndPayLocked(lock, id, client, idx, requestDigest, fulfilled, prover);
            } else {
                // NOTE: If the request is not priced, the context will be all zeroes. We will have
                // only reached this point if the request digest matches the lock, which is expired.
                // In this case, the price will be zero, which is correct.
                paymentError =
                    _fulfillAndPayWasLocked(lock, id, client, idx, context.price, requestDigest, fulfilled, prover);
            }
        } else {
            paymentError = _fulfillAndPayNeverLocked(id, client, idx, context.price, requestDigest, fulfilled, prover);
        }

        if (paymentError.length > 0) {
            emit PaymentRequirementsFailed(paymentError);
        }

        // `ProofDelivered` carries the legacy (pre-router) fulfillment shape — `id`/`requestDigest`
        // embedded inline — so the event's topic0 and payload stay decodable by clients that have
        // not upgraded their SDK. The current `Fulfillment` dropped those fields to save batch
        // calldata, so reconstruct the legacy shape here from the request identity and the fill.
        emit ProofDelivered(
            id,
            prover,
            LegacyFulfillment({
                id: id,
                requestDigest: requestDigest,
                claimDigest: fill.claimDigest,
                fulfillmentDataType: fill.fulfillmentDataType,
                fulfillmentData: fill.fulfillmentData,
                seal: fill.seal
            })
        );
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
        // NOTE: If the prover is paid, the fulfilled flag must be set.
        if (lock.isProverPaid()) {
            return abi.encodeWithSelector(RequestIsFulfilled.selector, RequestId.unwrap(id));
        }

        if (!fulfilled) {
            accounts[client].setRequestFulfilled(idx);
            emit RequestFulfilled(id, assessorProver, requestDigest);
        }

        // At this point the request has been fulfilled. The remaining logic determines whether
        // payment should be sent and to whom.
        // While the request is locked, only the locker is eligible for payment, and only for the request that was locked.
        if (lock.prover != assessorProver || lock.requestDigest != requestDigest) {
            return abi.encodeWithSelector(RequestIsLocked.selector, RequestId.unwrap(id));
        }
        requestLocks[id].setProverPaidBeforeLockDeadline();

        uint96 price = lock.price;
        if (MARKET_FEE_BPS > 0) {
            price = _applyMarketFee(price);
        }
        accounts[assessorProver].balance += price;
        accounts[assessorProver].collateralBalance += lock.collateral;
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
        uint96 price,
        bytes32 requestDigest,
        bool fulfilled,
        address assessorProver
    ) internal returns (bytes memory paymentError) {
        // NOTE: If the prover is paid, the fulfilled flag must be set.
        if (lock.isProverPaid()) {
            return abi.encodeWithSelector(RequestIsFulfilled.selector, RequestId.unwrap(id));
        }

        if (!fulfilled) {
            accounts[client].setRequestFulfilled(idx);
            emit RequestFulfilled(id, assessorProver, requestDigest);
        }

        // Deduct any additionally owned funds from client account. The client was already charged
        // for the price at lock time once when the request was locked. We only need to charge any
        // additional price for the difference between the price of the fulfilled request, at the
        // current block, and the price of the locked request.
        //
        // Note that although they have the same ID, the locked request and the fulfilled request
        // could be different. If the request fulfilled is the same as the one locked, the
        // price will be zero and the entire fee on the lock will be returned to the client.
        Account storage clientAccount = accounts[client];

        // If the request has the same id, but is different to the request that was locked, the fulfillment
        // price could be either higher or lower than the price that was previously locked.
        // If the price is higher, we charge the client the difference.
        // If the price is lower, we refund the client the difference.
        uint96 lockPrice = lock.price;
        bool partialPayment = false;
        uint96 finalPrice = price;

        if (price > lockPrice) {
            uint96 clientOwes = price - lockPrice;
            if (clientAccount.balance < clientOwes) {
                // If the client does not have enough balance to cover the full amount owed,
                // we will only charge them what they have available.
                clientOwes = clientAccount.balance;
                finalPrice = lockPrice + clientOwes;
                partialPayment = true;
            }
            unchecked {
                clientAccount.balance -= clientOwes;
            }
        } else {
            uint96 clientOwed = lockPrice - price;
            clientAccount.balance += clientOwed;
        }

        requestLocks[id].setProverPaidAfterLockDeadline(assessorProver);
        if (MARKET_FEE_BPS > 0) {
            finalPrice = _applyMarketFee(finalPrice);
        }
        accounts[assessorProver].balance += finalPrice;
        if (partialPayment) {
            return abi.encodeWithSelector(PartialPayment.selector, price, finalPrice);
        }
    }

    /// @notice For a request that has never been locked. Marks the request as fulfilled, and transfers payment if eligible.
    /// @dev If a never locked request is fulfilled, but client has not enough funds to cover the payment, no
    /// payment can ever be rendered for this order in the future.
    function _fulfillAndPayNeverLocked(
        RequestId id,
        address client,
        uint32 idx,
        uint96 price,
        bytes32 requestDigest,
        bool fulfilled,
        address assessorProver
    ) internal returns (bytes memory paymentError) {
        // When never locked, the fulfilled flag _does_ indicate that we alrady attempted to
        // transfer payment (which will only fail in the InsufficientBalance case below) so we
        // return early here.
        if (fulfilled) {
            return abi.encodeWithSelector(RequestIsFulfilled.selector, RequestId.unwrap(id));
        }

        Account storage clientAccount = accounts[client];
        clientAccount.setRequestFulfilled(idx);
        emit RequestFulfilled(id, assessorProver, requestDigest);

        // Deduct the funds from client account.
        // NOTE: In the case of InsufficientBalance, the payment can never be transferred in the
        // future. This is a simplifying choice.
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
        // Ensure sufficient gas for callback, accounting for EIP-150 (63/64 rule).
        // The requestor is responsible for ensuring that the callback gas limit is sufficient to cover
        // for any extra overhead that the caller pays (calldata copy, cold access, etc.).
        if (gasleft() * 63 / 64 < callbackGasLimit) revert InsufficientGas();
        try IBoundlessMarketCallback(callbackAddr).handleProof{gas: callbackGasLimit}(imageId, journal, seal) {}
        catch (bytes memory err) {
            emit CallbackFailed(id, callbackAddr, err);
        }
    }

    /// @inheritdoc IBoundlessMarket
    function submitRoot(address setVerifierAddress, bytes32 root, bytes calldata seal) external {
        _submitRoot(setVerifierAddress, root, seal);
    }

    /// @inheritdoc IBoundlessMarket
    function submitRootAndFulfill(
        address setVerifier,
        bytes32 root,
        bytes calldata seal,
        FulfillmentBatch[] calldata fulfillmentBatches
    ) external returns (bytes[] memory paymentError) {
        _submitRoot(setVerifier, root, seal);
        paymentError = fulfill(fulfillmentBatches);
    }

    /// @inheritdoc IBoundlessMarket
    function submitRootAndFulfillAndWithdraw(
        address setVerifier,
        bytes32 root,
        bytes calldata seal,
        FulfillmentBatch[] calldata fulfillmentBatches
    ) external returns (bytes[] memory paymentError) {
        _submitRoot(setVerifier, root, seal);
        paymentError = fulfillAndWithdraw(fulfillmentBatches);
    }

    /// @inheritdoc IBoundlessMarket
    function submitRootAndPriceAndFulfill(
        address setVerifier,
        bytes32 root,
        bytes calldata seal,
        ProofRequestBatch[] calldata requestBatches,
        FulfillmentBatch[] calldata fulfillmentBatches
    ) external returns (bytes[] memory paymentError) {
        _submitRoot(setVerifier, root, seal);
        paymentError = priceAndFulfill(requestBatches, fulfillmentBatches);
    }

    /// @inheritdoc IBoundlessMarket
    function submitRootAndPriceAndFulfillAndWithdraw(
        address setVerifier,
        bytes32 root,
        bytes calldata seal,
        ProofRequestBatch[] calldata requestBatches,
        FulfillmentBatch[] calldata fulfillmentBatches
    ) external returns (bytes[] memory paymentError) {
        _submitRoot(setVerifier, root, seal);
        paymentError = priceAndFulfillAndWithdraw(requestBatches, fulfillmentBatches);
    }

    /// @dev Shared dispatch for `submitRoot*` variants. Five call sites means
    ///      the optimizer should keep this factored instead of inlining the
    ///      external-call setup at each site.
    function _submitRoot(address setVerifier, bytes32 root, bytes calldata seal) private {
        IRiscZeroSetVerifier(setVerifier).submitMerkleRoot(root, seal);
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
        // does get fulfilled, we know which prover should receive a portion of the collateral.
        if (block.timestamp <= lock.deadline()) {
            revert RequestIsNotExpired({requestId: requestId, deadline: lock.deadline()});
        }

        // Request was either fulfilled after the lock deadline or the request expired unfulfilled.
        // In both cases the locker should be slashed.
        requestLocks[requestId].setSlashed();

        // Calculate the portion of collateral that should be burned vs sent to the prover.
        uint256 burnValue = uint256(lock.collateral) * SLASHING_BURN_BPS / 10000;

        // If a prover fulfilled the request after the lock deadline, that prover
        // receives the unburned portion of the collateral as a reward.
        // Otherwise the request expired unfulfilled, unburnt collateral accrues to the market treasury,
        // and we refund the client the price they paid for the request at lock time.
        uint96 transferValue = (uint256(lock.collateral) - burnValue).toUint96();
        address collateralRecipient = lock.prover;
        if (lock.isProverPaidAfterLockDeadline()) {
            // At this point lock.prover is the prover that ultimately fulfilled the request, not
            // the prover that locked the request. Transfer them the unburnt collateral.
            accounts[collateralRecipient].collateralBalance += transferValue;
        } else {
            collateralRecipient = address(this);
            accounts[collateralRecipient].collateralBalance += transferValue;
            accounts[client].balance += lock.price;
        }

        ERC20(COLLATERAL_TOKEN_CONTRACT).transfer(address(0xdEaD), burnValue);
        (burnValue);
        emit ProverSlashed(requestId, burnValue, transferValue, collateralRecipient);
    }

    /// @inheritdoc IBoundlessMarket
    function deposit() public payable {
        accounts[msg.sender].balance += msg.value.toUint96();
        emit Deposit(msg.sender, msg.value);
    }

    /// @inheritdoc IBoundlessMarket
    function depositTo(address to) public payable {
        accounts[to].balance += msg.value.toUint96();
        emit Deposit(to, msg.value);
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
    function depositCollateral(uint256 value) external {
        // Transfer tokens from user to market
        _depositCollateral(msg.sender, msg.sender, value);
    }

    /// @inheritdoc IBoundlessMarket
    function depositCollateralTo(address to, uint256 value) external {
        _depositCollateral(msg.sender, to, value);
    }

    /// @inheritdoc IBoundlessMarket
    function depositCollateralWithPermit(uint256 value, uint256 deadline, uint8 v, bytes32 r, bytes32 s) external {
        // Transfer tokens from user to market
        try ERC20(COLLATERAL_TOKEN_CONTRACT).permit(msg.sender, address(this), value, deadline, v, r, s) {} catch {}
        _depositCollateral(msg.sender, msg.sender, value);
    }

    /// @inheritdoc IBoundlessMarket
    function depositCollateralWithPermitTo(address to, uint256 value, uint256 deadline, uint8 v, bytes32 r, bytes32 s)
        external
    {
        try ERC20(COLLATERAL_TOKEN_CONTRACT).permit(msg.sender, address(this), value, deadline, v, r, s) {} catch {}
        _depositCollateral(msg.sender, to, value);
    }

    function _depositCollateral(address from, address to, uint256 value) internal {
        ERC20(COLLATERAL_TOKEN_CONTRACT).safeTransferFrom(from, address(this), value);
        accounts[to].collateralBalance += value.toUint96();
        emit CollateralDeposit(to, value);
    }

    /// @inheritdoc IBoundlessMarket
    function withdrawCollateral(uint256 value) public {
        if (accounts[msg.sender].collateralBalance < value.toUint96()) {
            revert InsufficientBalance(msg.sender);
        }
        unchecked {
            accounts[msg.sender].collateralBalance -= value.toUint96();
        }
        // Transfer tokens from market to user
        bool success = ERC20(COLLATERAL_TOKEN_CONTRACT).transfer(msg.sender, value);
        if (!success) revert TransferFailed();

        emit CollateralWithdrawal(msg.sender, value);
    }

    /// @inheritdoc IBoundlessMarket
    function balanceOfCollateral(address addr) public view returns (uint256) {
        return uint256(accounts[addr].collateralBalance);
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
        returns (bytes32, bytes32)
    {
        bytes32 eip712Digest = request.eip712Digest();
        bytes32 requestHash = _hashTypedDataV4(eip712Digest);
        if (request.id.isSmartContractSigned()) {
            if (
                IERC1271(addr).isValidSignature{gas: ERC1271_MAX_GAS_FOR_CHECK}(requestHash, clientSignature)
                    != IERC1271.isValidSignature.selector
            ) {
                revert IBoundlessMarket.InvalidSignature();
            }
        } else {
            if (ECDSA.recover(requestHash, clientSignature) != addr) {
                revert IBoundlessMarket.InvalidSignature();
            }
        }
        return (requestHash, eip712Digest);
    }

    /// @inheritdoc IBoundlessMarket
    function eip712DomainSeparator() external view returns (bytes32) {
        return _domainSeparatorV4();
    }
}
