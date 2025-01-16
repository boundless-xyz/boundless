// Copyright (c) 2025 RISC Zero, Inc.
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
import {IRiscZeroSetVerifier} from "risc0/IRiscZeroSetVerifier.sol";

import {IBoundlessMarket, ProofRequest, Offer, Fulfillment, AssessorJournal} from "./IBoundlessMarket.sol";
import {BoundlessMarketLib} from "./BoundlessMarketLib.sol";
import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {ERC20Burnable} from "@openzeppelin/contracts/token/ERC20/extensions/ERC20Burnable.sol";
import {IERC20Permit} from "@openzeppelin/contracts/token/ERC20/extensions/IERC20Permit.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";

uint256 constant REQUEST_FLAGS_BITWIDTH = 2;
uint256 constant REQUEST_FLAGS_INITIAL_BITS = 64;

/// @notice Account state is a combination of the account balance, and locked and fulfilled flags for requests.
struct Account {
    /// @dev uint96 is enough to represent the entire token supply of Ether.
    uint96 balance;
    /// @dev Balance of HP tokens.
    uint96 stakeBalance;
    /// @notice 32 pairs of 2 bits representing the status of a request. One bit is for lock-in and
    /// the other is for fulfillment.
    /// @dev Request state flags are packed into a uint64 to make balance and flags for the first
    /// 32 requests fit in one slot.
    uint64 requestFlagsInitial;
    /// @dev Flags for the remaining requests are in a storage array. Each uint256 holds the packed
    /// flags for 128 requests, indexed in a linear fashion. Note that this struct cannot be
    /// instantiated in memory.
    uint256[(1 << 32) * REQUEST_FLAGS_BITWIDTH / 256] requestFlagsExtended;
}

library AccountLib {
    /// Gets the locked and fulfilled request flags for the request with the given index.
    function requestFlags(Account storage account, uint32 idx) internal view returns (bool locked, bool fulfilled) {
        if (idx < REQUEST_FLAGS_INITIAL_BITS / REQUEST_FLAGS_BITWIDTH) {
            uint64 masked = (
                account.requestFlagsInitial
                    & (uint64((1 << REQUEST_FLAGS_BITWIDTH) - 1) << uint64(idx * REQUEST_FLAGS_BITWIDTH))
            ) >> (idx * REQUEST_FLAGS_BITWIDTH);
            return (masked & uint64(1) != 0, masked & uint64(2) != 0);
        } else {
            uint256 idxShifted = idx - (REQUEST_FLAGS_INITIAL_BITS / REQUEST_FLAGS_BITWIDTH);
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
        if (idx < REQUEST_FLAGS_INITIAL_BITS / REQUEST_FLAGS_BITWIDTH) {
            uint64 mask = uint64(flags) << uint64(idx * REQUEST_FLAGS_BITWIDTH);
            account.requestFlagsInitial |= mask;
        } else {
            uint256 idxShifted = idx - (REQUEST_FLAGS_INITIAL_BITS / REQUEST_FLAGS_BITWIDTH);
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
    // NOTE: There is another option here, which would be to have the request lock mapping index
    // based on request digest instead of index. As a friction, this would introduce a second
    // user-facing concept of what identifies a request.
    // NOTE: This fingerprint binds the full request including e.g. the offer and input. Technically,
    // all that is required is to bind the requirements. If there is some advantage to only binding
    // the requirements here (e.g. less hashing costs) then that might be worth doing.
    /// @notice Keccak256 hash of the request, shortened to 64-bits. During fulfillment, this value is used
    /// to check that the request completed is the request that was locked, and not some other
    /// request with the same ID.
    /// @dev Note that this value is not collision resistant in that it is fairly easy to find two
    /// requests with the same fingerprint. However, requests much be signed to be valid, and so
    /// the existence of two valid requests with the same fingerprint requires either intention
    /// construction by the private key holder, which would be pointless, or accidental collision.
    /// With 64-bits, a client that constructed 65k signed requests with the same request ID would
    /// have a roughly 2^-32 chance of accidental collision, which is negligible in this scenario.
    bytes8 fingerprint;
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

contract BoundlessMarket is
    IBoundlessMarket,
    Initializable,
    EIP712Upgradeable,
    Ownable2StepUpgradeable,
    UUPSUpgradeable
{
    using AccountLib for Account;
    using BoundlessMarketLib for Offer;
    using BoundlessMarketLib for ProofRequest;
    using ReceiptClaimLib for ReceiptClaim;
    using SafeCast for uint256;
    using TransientPriceLib for TransientPrice;
    using SafeERC20 for IERC20;

    /// @dev The version of the contract, with respect to upgrades.
    uint64 public constant VERSION = 1;

    // Mapping of request ID to lock-in state. Non-zero for requests that are locked in.
    mapping(uint256 => RequestLock) public requestLocks;
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

    /// @notice Mapping of addresses to frozen stake balances.
    /// @dev A frozen account cannot lock-in requests.
    mapping(address => uint96) public frozenAccounts;

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

    function requestIsFulfilled(uint256 id) external view returns (bool) {
        if (id & (uint256(type(uint64).max) << 192) != 0) {
            revert InvalidRequest();
        }
        (, bool fulfilled) = accounts[address(uint160(id >> 32))].requestFlags(uint32(id));
        return fulfilled;
    }

    function requestIsLocked(uint256 id) external view returns (bool) {
        if (id & (uint256(type(uint64).max) << 192) != 0) {
            revert InvalidRequest();
        }
        (bool locked,) = accounts[address(uint160(id >> 32))].requestFlags(uint32(id));
        return locked;
    }

    function requestDeadline(uint256 id) external view returns (uint64) {
        return requestLocks[id].deadline;
    }

    /// Domain separator for producing an EIP-712 signature to be verified by this contract.
    function eip712DomainSeparator() external view returns (bytes32) {
        return _domainSeparatorV4();
    }

    /// Internal method for verifying signatures over requests. Reverts on failure.
    function verifyRequestSignature(address addr, ProofRequest calldata request, bytes calldata signature)
        public
        view
        returns (bytes32 requestDigest)
    {
        bytes32 structHash = _hashTypedDataV4(request.eip712Digest());
        if (ECDSA.recover(structHash, signature) != addr) {
            revert InvalidSignature();
        }
        return structHash;
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
        if (!sent) {
            revert TransferFailed();
        }
        emit Withdrawal(msg.sender, value);
    }

    // Get the current balance of an account.
    function balanceOf(address addr) public view returns (uint256) {
        return uint256(accounts[addr].balance);
    }

    function _depositStake(address from, uint256 value) internal {
        IERC20(STAKE_TOKEN_CONTRACT).safeTransferFrom(from, address(this), value);
        accounts[from].stakeBalance += value.toUint96();
        emit StakeDeposit(from, value);
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

    // NOTE: We could verify the client signature here, but this adds about 18k gas (with a naive
    // implementation), doubling the cost of calling this method. It is not required for protocol
    // safety as the signature is checked during lockin, and during fulfillment (by the assessor).
    function submitRequest(ProofRequest calldata request, bytes calldata clientSignature) external payable {
        if (msg.value > 0) {
            deposit();
        }
        emit RequestSubmitted(request.id, request, clientSignature);
    }

    function lockin(ProofRequest calldata request, bytes calldata clientSignature) external {
        (address client, uint32 idx) =
            (BoundlessMarketLib.requestFrom(request.id), BoundlessMarketLib.requestIndex(request.id));
        (bytes32 requestDigest) = verifyRequestSignature(client, request, clientSignature);

        _lockinAuthed(request, requestDigest, client, idx, msg.sender);
    }

    function lockinWithSig(
        ProofRequest calldata request,
        bytes calldata clientSignature,
        bytes calldata proverSignature
    ) external {
        (address client, uint32 idx) =
            (BoundlessMarketLib.requestFrom(request.id), BoundlessMarketLib.requestIndex(request.id));

        // Recover the prover address and require the client address to equal the address part of the ID.
        bytes32 requestDigest = _hashTypedDataV4(request.eip712Digest());
        address prover = ECDSA.recover(requestDigest, proverSignature);
        if (ECDSA.recover(requestDigest, clientSignature) != client) {
            revert InvalidSignature();
        }

        _lockinAuthed(request, requestDigest, client, idx, prover);
    }

    /// Check that the request is valid, and not already locked or fulfilled by another prover.
    /// Returns the auction price and deadline for the request.
    function _validateRequestForLockin(ProofRequest calldata request, address client, uint32 idx)
        internal
        view
        returns (uint96 price, uint64 deadline)
    {
        // Check that the request is internally consistent and is not expired.
        request.offer.requireValid();

        // Check the deadline and compute the current price offered by the reverse Dutch auction.
        price = request.offer.priceAtBlock(uint64(block.number)).toUint96();
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

    function _lockinAuthed(
        ProofRequest calldata request,
        bytes32 requestDigest,
        address client,
        uint32 idx,
        address prover
    ) internal {
        (uint96 price, uint64 deadline) = _validateRequestForLockin(request, client, idx);

        // Deduct funds from the client account and prover HP account.
        Account storage clientAccount = accounts[client];
        if (clientAccount.balance < price) {
            revert InsufficientBalance(client);
        }
        Account storage proverAccount = accounts[prover];
        if (proverAccount.stakeBalance < request.offer.lockinStake.toUint96()) {
            revert InsufficientBalance(prover);
        }

        unchecked {
            clientAccount.balance -= price;
            proverAccount.stakeBalance -= request.offer.lockinStake.toUint96();
        }

        // Record the lock for the request and emit an event.
        requestLocks[request.id] = RequestLock({
            prover: prover,
            price: price,
            deadline: deadline,
            stake: request.offer.lockinStake.toUint96(),
            fingerprint: bytes8(requestDigest)
        });

        clientAccount.setRequestLocked(idx);
        emit RequestLockedin(request.id, prover);
    }

    /// @inheritdoc IBoundlessMarket
    function accountIsFrozen(address addr) external view returns (bool) {
        return _accountIsFrozen(addr);
    }

    /// @inheritdoc IBoundlessMarket
    function unfreezeAccount() public {
        address addr = msg.sender;
        if (_accountIsFrozen(addr)) {
            accounts[addr].stakeBalance += frozenAccounts[addr];
            frozenAccounts[addr] = 0;
        }
    }

    function _accountIsFrozen(address addr) internal view returns (bool) {
        return frozenAccounts[addr] > 0;
    }

    function freezeAccount(address addr) internal {
        if (!_accountIsFrozen(addr)) {
            frozenAccounts[addr] = accounts[addr].stakeBalance;
            accounts[addr].stakeBalance = 0;
        }
    }

    /// Validates the request and records the price to transient storage such that it can be
    /// fulfilled within the same transaction without taking a lock on it.
    function priceRequest(ProofRequest calldata request, bytes calldata clientSignature) public {
        (address client, uint32 idx) =
            (BoundlessMarketLib.requestFrom(request.id), BoundlessMarketLib.requestIndex(request.id));
        (bytes32 requestDigest) = verifyRequestSignature(client, request, clientSignature);

        (uint96 price,) = _validateRequestForLockin(request, client, idx);

        // Record the price in transient storage, such that the order can be filled in this same transaction.
        // NOTE: Since transient storage is cleared at the end of the transaction, we know that this
        // price will not become stale, and the request cannot expire, while this price is recorded.
        uint256 packed = TransientPrice({valid: true, price: price}).pack();
        assembly {
            tstore(requestDigest, packed)
        }
    }

    /// Verify the application and assessor receipts, ensuring that the provided fulfillment
    /// satisfies the request.
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

    /// Verify the application and assessor receipts for the batch, ensuring that the provided
    /// fulfillments satisfy the requests.
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

    function fulfill(Fulfillment calldata fill, bytes calldata assessorSeal, address prover) external {
        verifyDelivery(fill, assessorSeal, prover);
        _fulfillVerified(fill.id, fill.requestDigest, prover, fill.requirePayment);

        emit ProofDelivered(fill.id, fill.journal, fill.seal);
    }

    function fulfillBatch(Fulfillment[] calldata fills, bytes calldata assessorSeal, address prover) public {
        verifyBatchDelivery(fills, assessorSeal, prover);

        // NOTE: It would be slightly more efficient to keep balances and request flags in memory until a single
        // batch update to storage. However, updating the same storage slot twice only costs 100 gas, so
        // this savings is marginal, and will be outweighed by complicated memory management if not careful.
        for (uint256 i = 0; i < fills.length; i++) {
            _fulfillVerified(fills[i].id, fills[i].requestDigest, prover, fills[i].requirePayment);

            emit ProofDelivered(fills[i].id, fills[i].journal, fills[i].seal);
        }
    }

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

    /// Complete the fulfillment logic after having verified the app and assessor receipts.
    function _fulfillVerified(uint256 id, bytes32 requestDigest, address assessorProver, bool requirePayment)
        internal
    {
        address client = BoundlessMarketLib.requestFrom(id);
        uint32 idx = BoundlessMarketLib.requestIndex(id);

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
        uint256 id,
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
            return abi.encodeWithSelector(RequestIsFulfilled.selector, id);
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
            return abi.encodeWithSelector(RequestIsExpired.selector, id, lock.deadline);
        }
        if (lock.prover != assessorProver) {
            return abi.encodeWithSelector(RequestIsLocked.selector, id);
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
        uint256 id,
        address client,
        uint32 idx,
        bytes32 requestDigest,
        bool fulfilled,
        address assessorProver
    ) internal returns (bytes memory paymentError) {
        // When not locked, the fulfilled flag _does_ indicate that payment has already been transferred.
        if (fulfilled) {
            return abi.encodeWithSelector(RequestIsFulfilled.selector, id);
        }

        uint256 packed;
        assembly {
            packed := tload(requestDigest)
        }
        TransientPrice memory tprice = TransientPriceLib.unpack(packed);

        if (!tprice.valid) {
            return abi.encodeWithSelector(RequestIsNotPriced.selector, id);
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

    function slash(uint256 requestId) external {
        address client = BoundlessMarketLib.requestFrom(requestId);
        uint32 idx = BoundlessMarketLib.requestIndex(requestId);
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

        // Freeze the prover account.
        freezeAccount(lock.prover);

        // Zero out the lock to prevent the same request from being slashed twice.
        requestLocks[requestId] = RequestLock(address(0), uint96(0), uint64(0), uint96(0), bytes8(0));

        // Calculate the portion of stake that should be burned vs sent to the client.
        // NOTE: If the burn fraction is not properly set, this can overflow.
        // The maximum feasible stake multiplied by the numerator must be less than 2^256.
        uint256 burnValue = uint256(lock.stake) * SLASHING_BURN_FRACTION_NUMERATOR / SLASHING_BURN_FRACTION_DENOMINATOR;
        uint256 transferValue = uint256(lock.stake) - burnValue;

        // Return the price to the client, plus the transfer value. Then burn the burn value.
        accounts[client].balance += lock.price;
        accounts[client].stakeBalance += transferValue.toUint96();
        // Transfer tokens from market to address zero.
        ERC20Burnable(STAKE_TOKEN_CONTRACT).burn(burnValue);

        emit ProverSlashed(requestId, lock.prover, uint256(lock.stake));
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
        // TODO(#243): This will break when we change VERIFIER to point to the router.
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
        if (leaves.length == 0) {
            revert IBoundlessMarket.InvalidRequest();
        }

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
