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
import {ERC20} from "solmate/tokens/ERC20.sol";
import {SafeTransferLib} from "solmate/utils/SafeTransferLib.sol";
import {IERC1271} from "@openzeppelin/contracts/interfaces/IERC1271.sol";
import {IRiscZeroSetVerifier} from "risc0/IRiscZeroSetVerifier.sol";

import {IBoundlessMarket} from "./IBoundlessMarket.sol";
import {IBoundlessMarketCallback} from "./IBoundlessMarketCallback.sol";
import {Account} from "./types/Account.sol";
import {Fulfillment} from "./types/Fulfillment.sol";
import {FulfillmentDataLibrary, FulfillmentDataType} from "./types/FulfillmentData.sol";
import {ProofRequest} from "./types/ProofRequest.sol";
import {LockRequestLibrary} from "./types/LockRequest.sol";
import {RequestId} from "./types/RequestId.sol";
import {RequestLock} from "./types/RequestLock.sol";
import {ProofRequestBatch} from "./types/ProofRequestBatch.sol";
import {SlimRequest, SlimRequestLibrary} from "./types/SlimRequest.sol";
import {FulfillmentBatch} from "./types/FulfillmentBatch.sol";
import {FulfillmentContext, FulfillmentContextLibrary} from "./types/FulfillmentContext.sol";

import {BoundlessMarketLib} from "./libraries/BoundlessMarketLib.sol";

import {IBoundlessRouter} from "./router/interfaces/IBoundlessRouter.sol";

import {FulfillLib} from "./FulfillLib.sol";

error InvalidRouter();
error InvalidCollateralToken();
error InvalidLegacyImpl();
error InvalidFulfillLib();
error InvalidInitialOwner();
error MismatchedRequestId(uint256 expected, uint256 received);

contract BoundlessMarket is
    IBoundlessMarket,
    Initializable,
    EIP712Upgradeable,
    AccessControlUpgradeable,
    UUPSUpgradeable
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

    /// @notice Address of the FulfillLib contract that holds the extracted
    ///         fulfill chain. The new-ABI entrypoints delegatecall this impl;
    ///         the lib mirrors this contract's storage layout so it
    ///         reads/writes the same slots.
    /// @custom:oz-upgrades-unsafe-allow state-variable-immutable
    address public immutable FULFILL_LIB;

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

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor(IBoundlessRouter router, address collateralTokenContract, address legacyImpl, address fulfillLib) {
        if (address(router) == address(0)) revert InvalidRouter();
        if (collateralTokenContract == address(0)) revert InvalidCollateralToken();
        if (legacyImpl == address(0)) revert InvalidLegacyImpl();
        if (fulfillLib == address(0)) revert InvalidFulfillLib();

        ROUTER = router;
        COLLATERAL_TOKEN_CONTRACT = collateralTokenContract;
        LEGACY_IMPL = legacyImpl;
        FULFILL_LIB = fulfillLib;

        _disableInitializers();
    }

    /// @notice Forwards any selector not declared on this contract to the
    ///         previous implementation via delegate-call, preserving the
    ///         caller, value, and the proxy's storage context.
    /// @dev    Used to keep the legacy ABI surface live during the migration
    ///         window without re-introducing the legacy bodies into this
    ///         implementation's bytecode.
    fallback() external payable {
        address impl = LEGACY_IMPL;
        assembly {
            calldatacopy(0, 0, calldatasize())
            let result := delegatecall(gas(), impl, 0, calldatasize(), 0, 0)
            returndatacopy(0, 0, returndatasize())
            switch result
            case 0 { revert(0, returndatasize()) }
            default { return(0, returndatasize()) }
        }
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

    /// @inheritdoc IBoundlessMarket
    function priceAndFulfill(
        ProofRequestBatch[] calldata requestBatches,
        FulfillmentBatch[] calldata fulfillmentBatches
    ) external returns (bytes[] memory) {
        _priceAll(requestBatches);
        _delegateFulfill(fulfillmentBatches); // terminates
    }

    /// @inheritdoc IBoundlessMarket
    function fulfill(FulfillmentBatch[] calldata fulfillmentBatches) external returns (bytes[] memory) {
        _delegateFulfill(fulfillmentBatches); // terminates
    }

    /// @dev Single delegatecall site to FulfillLib. Builds the lib's calldata
    ///      directly via assembly — `selector | offset(32) | <FulfillmentBatch[]
    ///      length + elements>` — by copying the batches blob out of our own
    ///      calldata. This avoids the bytecode cost of Solidity's general-purpose
    ///      `abi.encodeCall` encoder. On return, bubbles `returndata` as the
    ///      outer function's ABI response.
    function _delegateFulfill(FulfillmentBatch[] calldata fulfillmentBatches) private {
        address lib = FULFILL_LIB;
        bytes4 sel = FulfillLib.fulfill.selector;
        assembly {
            let lengthPos := sub(fulfillmentBatches.offset, 32)
            let dataLen := sub(calldatasize(), lengthPos)
            mstore(0, sel)
            mstore(4, 0x20)
            calldatacopy(36, lengthPos, dataLen)
            let ok := delegatecall(gas(), lib, 0, add(36, dataLen), 0, 0)
            returndatacopy(0, 0, returndatasize())
            switch ok
            case 0 { revert(0, returndatasize()) }
            default { return(0, returndatasize()) }
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
    ) external returns (bytes[] memory) {
        _submitRoot(setVerifier, root, seal);
        _delegateFulfill(fulfillmentBatches); // terminates
    }

    /// @inheritdoc IBoundlessMarket
    function submitRootAndPriceAndFulfill(
        address setVerifier,
        bytes32 root,
        bytes calldata seal,
        ProofRequestBatch[] calldata requestBatches,
        FulfillmentBatch[] calldata fulfillmentBatches
    ) external returns (bytes[] memory) {
        _submitRoot(setVerifier, root, seal);
        _priceAll(requestBatches);
        _delegateFulfill(fulfillmentBatches); // terminates
    }

    /// @dev Shared dispatch for `submitRoot*` variants.
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
