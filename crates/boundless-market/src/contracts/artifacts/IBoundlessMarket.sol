// Copyright 2024 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

pragma solidity ^0.8.20;

// TODO(#159) Think about compressing this struct. One way to reduce
// associated gas costs would be to put all the fields not needed for lockin
// into a sub-struct that is hashed.
struct ProofRequest {
    /// @notice Unique ID for this request, constructed from the client address and a 32-bit index.
    /// Constructed as (address(client) << 32) | index.
    /// @dev Note that the high-order 64 bits of this ID are currently unused and must set to zero.
    /// In the future it may be used to encode a version number and/or other flags.
    uint256 id;
    /// Requirements of the delivered proof. Specifies the program that must be run, and constrains
    /// value of the journal, specifying the statement that is requesting to be proven.
    Requirements requirements;
    /// A public URI where the program (i.e. image) can be downloaded. This URI will be accessed by
    /// provers that are evaluating whether to bid on the request.
    string imageUrl;
    /// Input to be provided to the zkVM guest execution.
    Input input;
    /// Offer specifying how much the client is willing to pay to have this request fulfilled.
    Offer offer;
}

struct Requirements {
    bytes32 imageId;
    Predicate predicate;
}

struct Predicate {
    PredicateType predicateType;
    bytes data;
}

enum PredicateType {
    DigestMatch,
    PrefixMatch
}

struct Input {
    InputType inputType;
    bytes data;
}

enum InputType {
    Inline,
    Url
}

struct Offer {
    /// Price at the start of the bidding period, it is minimum price a prover will receive for job.
    uint256 minPrice;
    /// Price at the end of the bidding period, this is the maximum price the client will pay.
    uint256 maxPrice;
    /// Block number at which bidding starts.
    uint64 biddingStart;
    /// Length of the "ramp-up period," measured in blocks since bidding start.
    /// Once bidding starts, the price begins to "ramp-up." During this time,
    /// the price rises each block until it reaches maxPrice.
    uint32 rampUpPeriod;
    /// Timeout for delivering the proof, expressed as a number of blocks from bidding start.
    /// Once locked-in, if a valid proof is not submitted before this deadline,
    /// the prover can be "slashed," which refunds the price to the requester.
    uint32 timeout;
    /// Bidders must stake this amount as part of their bid.
    uint256 lockinStake;
}

/// Info posted by the prover to fulfill a request, and get paid.
struct Fulfillment {
    /// ID of the request that is being fulfilled.
    uint256 id;
    /// EIP-712 digest of request struct.
    bytes32 requestDigest;
    /// Image ID of the guest that was verifiably executed to satisfy the request.
    /// Must match the value in the request's requirements.
    bytes32 imageId;
    // TODO(victor): Add a flag in the request to decide whether to post the journal. Note that
    // if the journal and journal digest do not need to be delivered to the client, imageId will
    // be replaced with claim digest, since it is captured in the requirements on the request,
    // checked by the Assessor guest.
    /// Journal committed by the guest program execution. The journal is checked to satisfy the
    /// predicate specified on the request's requirements.
    bytes journal;
    /// Cryptographic proof for the validity of the execution results. This will be sent to
    /// the `IRiscZeroVerifier` associated with this contract.
    bytes seal;
    /// If true, the fulfill operation will revert if there is any error that prevents the transfer
    /// of payment. If false, the transaction will not revert, but a `PaymentRequirementsFailed`
    /// event will be logged. This is useful to avoid reverting an entire batch of fulfillments if
    /// one fails.
    ///
    /// Note that setting this to `false` does not _prevent_ payment from being transferred.
    bool requirePayment;
}

/// Structured journal of the Assessor guest which verifies the signature(s)
/// from client(s) and that the requirements are met by claim digest(s) in the
/// Merkle tree committed to by the given root. Assessor can verify a batch of
/// requests, including batches of size one.
struct AssessorJournal {
    /// @notice Digest of each request validated by the assessor.
    /// @dev When a client signs two requests with the same ID, only one can ever be fulfilled.
    /// Using the digest here ensures that the request validated by the assessor matches the one
    /// that was locked / priced.
    bytes32[] requestDigests;
    // Root of the Merkle tree committing to the set of proven claims.
    // In the case of a batch of size one, this may simply be a claim digest.
    bytes32 root;
    // The address of the prover that produced the assessor receipt.
    address prover;
}

interface IBoundlessMarket {
    /// @notice Event logged when a new proof request is submitted by a client.
    /// @dev Note that the signature is not verified by the contract and should instead be verified
    ///      by the receiver of the event.
    event RequestSubmitted(uint256 indexed requestId, ProofRequest request, bytes clientSignature);
    /// Event logged when a request is locked in by the given prover.
    event RequestLockedin(uint256 indexed requestId, address prover);
    /// Event logged when a request is fulfilled.
    event RequestFulfilled(uint256 indexed requestId);
    /// @notice Event logged when a proof is delivered that satisfies the requests requirements.
    /// @dev It is possible for this event to be logged multiple times for a single request. This
    /// is usually logged as part of order fulfillment, however it can also be logged by a prover
    /// sending the proof without payment.
    event ProofDelivered(uint256 indexed requestId, bytes journal, bytes seal);
    /// Event when prover stake is slashed for failing to fulfill a request by the deadline.
    /// Part of the stake is burned, and part is transferred to the client as compensation.
    event ProverSlashed(uint256 indexed requestId, uint256 stakeBurned, uint256 stakeTransferred);
    /// Event when a deposit is made to the market.
    event Deposit(address indexed account, uint256 value);
    /// Event when a withdrawal is made from the market.
    event Withdrawal(address indexed account, uint256 value);
    /// Contract upgraded to a new version.
    event Upgraded(uint64 indexed version);
    /// @notice Event emitted during fulfillment if a request was fulfilled, but payment was not
    /// transferred because at least one condition was not met. See the documentation on
    /// `IBoundlessMarket.fulfillBatch` for more information.
    ///
    /// If there is an unexpired lock on the request, the order, the prover holding the lock may
    /// still be able to / transfer payment sending another transaction.
    ///
    /// @dev The payload of the event is an ABI encoded error, from the errors on this contract.
    event PaymentRequirementsFailed(bytes error);

    /// Request is locked when it was not required to be.
    error RequestIsLocked(uint256 requestId);
    /// Request is not priced when it was required to be. Either locking the request, or calling the
    /// `IBoundlessMarket.priceRequest` function in the same transaction will satisfy this requirement.
    error RequestIsNotPriced(uint256 requestId);
    /// Request is not locked when it was required to be.
    error RequestIsNotLocked(uint256 requestId);
    /// Request is fulfilled when it was not required to be.
    error RequestIsFulfilled(uint256 requestId);
    /// Request is no longer valid, as the deadline has passed.
    error RequestIsExpired(uint256 requestId, uint64 deadline);
    /// Request is still valid, as the deadline has yet to pass.
    error RequestIsNotExpired(uint256 requestId, uint64 deadline);
    /// @notice Request fingerprint (shortened digest) doesn't match the value that is locked.
    /// @dev This can happen if a client signs multiple requests with the same ID (i.e. multiple
    /// versions of the same request) and a prover locks one version but the tries to call fulfill
    /// using a different version.
    error RequestLockFingerprintDoesNotMatch(uint256 requestId, bytes8 provided, bytes8 locked);
    /// Unable to complete request because of insufficient balance.
    error InsufficientBalance(address account);
    /// A signature did not pass verification checks.
    error InvalidSignature();
    /// Request is malformed or internally inconsistent.
    error InvalidRequest();
    /// Error when transfer of funds to an external address fails.
    error TransferFailed();

    /// @notice Check if the given request has been locked (i.e. accepted) by a prover.
    /// @dev When a request is locked, only the prover it is locked to can be paid to fulfill the job.
    function requestIsLocked(uint256 requestId) external view returns (bool);
    /// @notice Check if the given request has been fulfilled (i.e. a proof was delivered).
    function requestIsFulfilled(uint256 requestId) external view returns (bool);
    /// @notice Return when the given request expires.
    function requestDeadline(uint256 requestId) external view returns (uint64);

    /// @notice Deposit Ether into the market to pay for proof and/or lockin stake.
    /// @dev Value deposited is msg.value and it is credited to the account of msg.sender.
    function deposit() external payable;
    /// @notice Withdraw Ether from the market.
    /// @dev Value is debited from msg.sender.
    function withdraw(uint256 value) external;
    /// @notice Check the deposited balance, in Ether, of the given account.
    function balanceOf(address addr) external view returns (uint256);

    /// @notice Submit a request such that it is publicly available for provers to evaluate and bid on.
    ///         Any `msg.value` sent with the call will be added to the balance of `msg.sender`.
    /// @dev Submitting the transaction only broadcasting it, and is not a required step.
    ///      This method does not validate the signature or store any state related to the request.
    function submitRequest(ProofRequest calldata request, bytes calldata clientSignature) external payable;

    /// @notice Lock the request to the prover, giving them exclusive rights to be paid to
    /// fulfill this request, and also making them subject to slashing penalties if they fail to
    /// deliver. At this point, the price for fulfillment is also set, based on the reverse Dutch
    /// auction parameters and the block at which this transaction is processed.
    /// @dev This method should be called from the address of the prover.
    function lockin(ProofRequest calldata request, bytes calldata clientSignature) external;

    /// @notice Lock the request to the prover, giving them exclusive rights to be paid to
    /// fulfill this request, and also making them subject to slashing penalties if they fail to
    /// deliver. At this point, the price for fulfillment is also set, based on the reverse Dutch
    /// auction parameters and the block at which this transaction is processed.
    /// @dev This method uses the provided signature to authenticate the prover.
    function lockinWithSig(
        ProofRequest calldata request,
        bytes calldata clientSignature,
        bytes calldata proverSignature
    ) external;

    /// @notice Fulfill a request by delivering the proof for the application.
    ///
    /// If the order is locked, only the prover that locked the order may receive payment.
    /// If another prover delivers a proof for an order that is locked, this method will revert
    /// unless `paymentRequired` is set to `false` on the `Fulfillment` struct.
    ///
    /// @param fill The fulfillment information, including the journal and seal.
    /// @param assessorSeal The seal from the Assessor guest, which is verified to confirm the
    /// request's requirements are met.
    /// @param prover The address of the prover that produced the fulfillment.
    /// Note that this can differ from the address of the prover that locked the
    /// request. Only the locked-in prover can receive payment.
    function fulfill(Fulfillment calldata fill, bytes calldata assessorSeal, address prover) external;
    /// @notice Fulfills a batch of requests. See IBoundlessMarket.fulfill for more information.
    function fulfillBatch(Fulfillment[] calldata fills, bytes calldata assessorSeal, address prover) external;

    /// @notice Checks the validity of the request and then writes the current auction price to
    /// transient storage.
    /// @dev When called within the same transaction, this method can be used to fulfill a request
    /// that is not locked. This is useful when the prover wishes to fulfill a request, but does
    /// not want to issue a lock transaction e.g. because the stake is to high or to save money by
    /// avoiding the gas costs of the lock transaction.
    function priceRequest(ProofRequest calldata request, bytes calldata clientSignature) external;

    /// @notice A combined call to `IBoundlessMarket.priceRequest` and `IBoundlessMarket.fulfillBatch`.
    /// The caller should provide the signed request and signature for each unlocked request they
    /// want to fulfill. Payment for unlocked requests will go to the provided `prover` address.
    function priceAndFulfillBatch(
        ProofRequest[] calldata requests,
        bytes[] calldata clientSignatures,
        Fulfillment[] calldata fills,
        bytes calldata assessorSeal,
        address prover
    ) external;

    /// @notice Combined function to submit a new merkle root to the set-verifier and call fulfillBatch.
    /// @dev Useful to reduce the transaction count for fulfillments
    function submitRootAndFulfillBatch(
        bytes32 root,
        bytes calldata seal,
        Fulfillment[] calldata fills,
        bytes calldata assessorSeal,
        address prover
    ) external;

    /// When a prover fails to fulfill a request by the deadline, this method can be used to burn
    /// the associated prover stake.
    function slash(uint256 requestId) external;

    /// EIP 712 domain separator getter
    function eip712DomainSeparator() external view returns (bytes32);

    /// Returns the assessor imageId and its url.
    function imageInfo() external view returns (bytes32, string memory);
}
