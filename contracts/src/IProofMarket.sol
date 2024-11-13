// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

pragma solidity ^0.8.20;

// TODO(victor) Think about compressing this struct. One way to reduce
// associated gas costs would be to put all the fields not needed for lockin
// into a sub-struct that is hashed.
struct ProvingRequest {
    /// Unique ID for this request, constructed from the client address and a 32-bit index.
    /// Constructed as (address(client) << 32) | index.
    uint192 id;
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
    // Price at the start of the bidding period, it is minimum price a prover will receive for job.
    uint96 minPrice;
    // Price at the end of the bidding period, this is the maximum price the client will pay.
    uint96 maxPrice;
    // Block number at which bidding starts.
    uint64 biddingStart;
    // Length of the "ramp-up period," measured in blocks.
    // Once bidding starts, the price begins to "ramp-up." During this time,
    // the price rises each block until it reaches maxPrice.
    uint32 rampUpPeriod;
    // Timeout for delivering the proof, expressed as a number of blocks from bidding start.
    // Once locked-in, if a valid proof is not submitted before this deadline,
    // the prover can be "slashed," which refunds the price to the requester.
    uint32 timeout;
    // Bidders must stake this amount as part of their bid.
    uint96 lockinStake;
}

// Info posted by the prover to fulfill a request, and get paid.
struct Fulfillment {
    uint192 id;
    bytes32 imageId;
    // TODO(victor): Add a flag in the request to decide whether to post the journal. Note that
    // if the journal and journal digest do not need to be delivered to the client, imageId will
    // be replaced with claim digest, since it is captured in the requirements on the request,
    // checked by the Assessor guest.
    bytes journal;
    bytes seal;
}

// Structured journal of the Assessor guest which verifies the signature(s)
// from client(s) and that the requirements are met by claim digest(s) in the
// Merkle tree committed to by the given root. Assessor can verify a batch of
// requests, including batches of size one.
struct AssessorJournal {
    uint192[] requestIds;
    // Root of the Merkle tree committing to the set of proven claims.
    // In the case of a batch of size one, this may simply be a claim digest.
    bytes32 root;
    // EIP712 domain separator.
    bytes32 eip712DomainSeparator;
    // The address of the prover that produced the assessor receipt.
    address prover;
}

interface IProofMarket {
    /// Event logged when a new proving request is submitted by a client.
    event RequestSubmitted(ProvingRequest request, bytes clientSignature);
    /// Event logged when a request is locked in by the given prover.
    event RequestLockedin(uint192 indexed requestId, address prover);
    /// Event logged when a request is fulfilled.
    event RequestFulfilled(uint192 indexed requestId);
    /// @notice Event logged when a proof is delivered that satisfies the requests requirements.
    /// @dev It is possible for this event to be logged multiple times for a single request. This
    /// is usually logged as part of order fulfillment, however it can also be logged by a prover
    /// sending the proof without payment.
    event ProofDelivered(uint192 indexed requestId, bytes journal, bytes seal);
    /// Event when prover stake is slashed for failing to fulfill a request by the deadline.
    /// Part of the stake is burned, and part is transferred to the client as compensation.
    event ProverSlashed(uint192 indexed requestId, uint256 stakeBurned, uint256 stakeTransferred);
    /// Event when a deposit is made to the proof market.
    event Deposit(address indexed account, uint256 value);
    /// Event when a withdrawal is made from the proof market.
    event Withdrawal(address indexed account, uint256 value);
    /// Contract upgraded to a new version.
    event Upgraded(uint64 indexed version);

    /// Request is locked when it was not expected to be.
    error RequestIsLocked(uint192 requestId);
    /// Request is not locked when it was expected to be.
    error RequestIsNotLocked(uint192 requestId);
    /// Request is fulfilled when it was not expected to be.
    error RequestIsFulfilled(uint192 requestId);
    /// Request is no longer valid, as the deadline has passed.
    error RequestIsExpired(uint192 requestId, uint64 deadline);
    /// Request is still valid, as the deadline has yet to pass.
    error RequestIsNotExpired(uint192 requestId, uint64 deadline);
    /// Unable to complete request because of insufficient balance.
    error InsufficientBalance(address account);
    /// Request has been slashed already.
    error RequestAlreadySlashed(uint192 requestId);

    /// @notice Check if the given request has been locked (i.e. accepted) by a prover.
    /// @dev When a request is locked, only the prover it is locked to can be paid to fulfill the job.
    function requestIsLocked(uint192 requestId) external view returns (bool);
    /// @notice Check if the given request has been fulfilled (i.e. a proof was delivered).
    function requestIsFulfilled(uint192 requestId) external view returns (bool);
    /// @notice Return when the given request expires.
    function requestDeadline(uint192 requestId) external view returns (uint64);

    /// @notice Deposit Ether into the proof market to pay for proof and/or lockin stake.
    /// @dev Value deposited is msg.value and it is credited to the account of msg.sender.
    function deposit() external payable;
    /// @notice Withdraw Ether from the proof market.
    /// @dev Value is debited from msg.sender.
    function withdraw(uint256 value) external;
    /// @notice Check the deposited balance, in Ether, of the given account.
    function balanceOf(address addr) external view returns (uint256);

    /// @notice Submit a request such that it is publicly available for provers to evaluate and bid on.
    ///         Any `msg.value` sent with the call will be added to the balance of `msg.sender`.
    /// @dev Submitting the transaction only broadcasting it, and is not a required step.
    function submitRequest(ProvingRequest calldata request, bytes calldata clientSignature) external payable;

    /// @notice Lock the proving request to the prover, giving them exclusive rights to be paid to
    /// fulfill this request, and also making them subject to slashing penalties if they fail to
    /// deliver. At this point, the price for fulfillment is also set, based on the reverse Dutch
    /// auction parameters and the block at which this transaction is processed.
    /// @dev This method should be called from the address of the prover.
    function lockin(ProvingRequest calldata request, bytes calldata clientSignature) external;

    /// @notice Lock the proving request to the prover, giving them exclusive rights to be paid to
    /// fulfill this request, and also making them subject to slashing penalties if they fail to
    /// deliver. At this point, the price for fulfillment is also set, based on the reverse Dutch
    /// auction parameters and the block at which this transaction is processed.
    /// @dev This method uses the provided signature to authenticate the prover.
    function lockinWithSig(
        ProvingRequest calldata request,
        bytes calldata clientSignature,
        bytes calldata proverSignature
    ) external;

    /// @notice Fulfill a locked request by delivering the proof for the application.
    /// Upon proof verification, the prover that locked the request will be paid.
    /// @param fill The fulfillment information, including the journal and seal.
    /// @param assessorSeal The seal from the Assessor guest, which is verified to confirm the
    /// request's requirements are met.
    /// @param prover The address of the prover that produced the fulfillment.
    /// Note that this can differ from the address of the prover that locked the
    /// request. When they differ, the locked-in prover is the one that received payment.
    function fulfill(Fulfillment calldata fill, bytes calldata assessorSeal, address prover) external;
    /// @notice Fulfills a batch of locked requests. See IProofMarket.fulfill for more information.
    function fulfillBatch(Fulfillment[] calldata fills, bytes calldata assessorSeal, address prover) external;

    /// @notice Checks the validity of the request and then writes the current auction price to
    /// transient storage.
    /// @dev When called within the same transaction, this method can be used to fulfill a request
    /// that is not locked. This is useful when the prover wishes to fulfill a request, but does
    /// not want to issue a lock transaction e.g. because the stake is to high or to save money by
    /// avoiding the gas costs of the lock transaction.
    function priceRequest(ProvingRequest calldata request, bytes calldata clientSignature) external;

    /// @notice A combined call to `IProofMarket.priceRequest` and `IProofMarket.fulfillBatch`.
    /// The caller should provide the signed request and signature for each unlocked request they
    /// want to fulfill. Payment for unlocked requests will go to the provided `prover` address.
    function priceAndFulfillBatch(
        ProvingRequest[] calldata requests,
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
    function slash(uint192 requestId) external;

    /// EIP 712 domain separator getter
    function eip712DomainSeparator() external view returns (bytes32);

    /// Returns the assessor imageId and its url.
    function imageInfo() external view returns (bytes32, string memory);
}
