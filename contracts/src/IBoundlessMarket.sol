// Copyright 2025 RISC Zero, Inc.
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

import {Fulfillment} from "./types/Fulfillment.sol";
import {AssessorReceipt} from "./types/AssessorReceipt.sol";
import {ProofRequest} from "./types/ProofRequest.sol";
import {RequestId} from "./types/RequestId.sol";

interface IBoundlessMarket {
    /// @notice Event logged when a new proof request is submitted by a client.
    /// @dev Note that the signature is not verified by the contract and should instead be verified
    /// by the receiver of the event.
    /// @param requestId The ID of the request.
    event RequestSubmitted(RequestId indexed requestId);

    /// @notice Event logged when a request is locked in by the given prover.
    /// @param requestId The ID of the request.
    /// @param prover The address of the prover.
    event RequestLocked(RequestId indexed requestId, address prover);

    /// @notice Event logged when a request is fulfilled.
    /// @param requestId The ID of the request.
    event RequestFulfilled(RequestId indexed requestId);

    /// @notice Event logged when a proof is delivered that satisfies the request's requirements.
    /// @dev It is possible for this event to be logged multiple times for a single request. This
    /// is usually logged as part of order fulfillment, however it can also be logged by a prover
    /// sending the proof without payment.
    /// @param requestId The ID of the request.
    event ProofDelivered(RequestId indexed requestId);

    /// Event when a prover is slashed is made to the market.
    /// @param requestId The ID of the request.
    /// @param stakeBurned The amount of stake burned.
    /// @param stakeTransferred The amount of stake transferred to either the fulfilling prover or the market.
    /// @param stakeRecipient The address of the stake recipient. Typically the fulfilling prover, but can be the market.
    event ProverSlashed(
        RequestId indexed requestId, uint256 stakeBurned, uint256 stakeTransferred, address stakeRecipient
    );

    /// @notice Event when a deposit is made to the market.
    /// @param account The account making the deposit.
    /// @param value The value of the deposit.
    event Deposit(address indexed account, uint256 value);

    /// @notice Event when a withdrawal is made from the market.
    /// @param account The account making the withdrawal.
    /// @param value The value of the withdrawal.
    event Withdrawal(address indexed account, uint256 value);
    /// @notice Event when a stake deposit is made to the market.
    /// @param account The account making the deposit.
    /// @param value The value of the deposit.
    event StakeDeposit(address indexed account, uint256 value);
    /// @notice Event when a stake withdrawal is made to the market.
    /// @param account The account making the withdrawal.
    /// @param value The value of the withdrawal.
    event StakeWithdrawal(address indexed account, uint256 value);

    /// @notice Event when the contract is upgraded to a new version.
    /// @param version The new version of the contract.
    event Upgraded(uint64 indexed version);

    /// @notice Event emitted when a callback to a contract fails during fulfillment
    /// @param requestId The ID of the request that was being fulfilled
    /// @param callback The address of the callback contract that failed
    /// @param error The error message from the failed call
    event CallbackFailed(RequestId indexed requestId, address callback, bytes error);

    /// @notice Error when a request is locked when it was not required to be.
    /// @param requestId The ID of the request.
    error RequestIsLocked(RequestId requestId);

    /// @notice Error when a request is not priced when it was required to be. Either locking the request, or calling the
    /// `IBoundlessMarket.priceRequest` function in the same transaction will satisfy this requirement.
    /// @param requestId The ID of the request.
    error RequestIsNotPriced(RequestId requestId);

    /// @notice Error when a request is not locked when it was required to be.
    /// @param requestId The ID of the request.
    error RequestIsNotLocked(RequestId requestId);

    /// @notice Error when a request is fulfilled when it was not required to be.
    /// @param requestId The ID of the request.
    error RequestIsFulfilled(RequestId requestId);

    /// @notice Error when a request is slashed when it was not required to be.
    /// @param requestId The ID of the request.
    error RequestIsSlashed(RequestId requestId);

    /// @notice Error when a request is no longer valid, as the deadline has passed.
    /// @param requestId The ID of the request.
    /// @param deadline The deadline of the request.
    error RequestIsExpired(RequestId requestId, uint64 deadline);

    /// @notice Error when a request is still valid, as the deadline has yet to pass.
    /// @param requestId The ID of the request.
    /// @param deadline The deadline of the request.
    error RequestIsNotExpired(RequestId requestId, uint64 deadline);

    /// @notice Error when request fingerprint (shortened digest) doesn't match the value that is locked.
    /// @dev This can happen if a client signs multiple requests with the same ID (i.e. multiple
    /// versions of the same request) and a prover locks one version but then tries to call fulfill
    /// using a different version.
    /// @param requestId The ID of the request.
    /// @param provided The provided fingerprint.
    /// @param locked The locked fingerprint.
    error RequestLockFingerprintDoesNotMatch(RequestId requestId, bytes8 provided, bytes8 locked);

    /// @notice Error when unable to complete request because of insufficient balance.
    /// @param account The account with insufficient balance.
    error InsufficientBalance(address account);

    /// @notice Error when a signature did not pass verification checks.
    error InvalidSignature();

    /// @notice Error when a request is malformed or internally inconsistent.
    error InvalidRequest();

    /// @notice Error when transfer of funds to an external address fails.
    error TransferFailed();

    /// @notice Error when providing a seal with a different selector than required.
    error SelectorMismatch(bytes4 required, bytes4 provided);

    /// @notice Error when the batch size exceeds the limit.
    error BatchSizeExceedsLimit(uint256 batchSize, uint256 limit);

    /// @notice Check if the given request has been locked (i.e. accepted) by a prover.
    /// @dev When a request is locked, only the prover it is locked to can be paid to fulfill the job.
    /// @param requestId The ID of the request.
    /// @return True if the request is locked, false otherwise.
    function requestIsLocked(RequestId requestId) external view returns (bool);

    /// @notice Check if the given request resulted in the prover being slashed
    /// (i.e. request was locked in but proof was not delivered)
    /// @dev Note it is possible for a request to result in a slash, but still be fulfilled
    /// if for example another prover decided to fulfill the request altruistically.
    /// This function should not be used to determine if a request was fulfilled.
    /// @param requestId The ID of the request.
    /// @return True if the request resulted in the prover being slashed, false otherwise.
    function requestIsSlashed(RequestId requestId) external view returns (bool);

    /// @notice Check if the given request has been fulfilled (i.e. a proof was delivered).
    /// @param requestId The ID of the request.
    /// @return True if the request is fulfilled, false otherwise.
    function requestIsFulfilled(RequestId requestId) external view returns (bool);

    /// @notice For a given locked request, returns when the lock expires.
    /// @dev If the request is not locked, this function will revert.
    /// @param requestId The ID of the request.
    /// @return The expiration time of the lock on the request.
    function requestLockDeadline(RequestId requestId) external view returns (uint64);

    /// @notice For a given locked request, returns when request expires.
    /// @dev If the request is not locked, this function will revert.
    /// @param requestId The ID of the request.
    /// @return The expiration time of the request.
    function requestDeadline(RequestId requestId) external view returns (uint64);

    /// @notice Deposit Ether into the market to pay for proof.
    /// @dev Value deposited is msg.value and it is credited to the account of msg.sender.
    function deposit() external payable;

    /// @notice Withdraw Ether from the market.
    /// @dev Value is debited from msg.sender.
    /// @param value The amount to withdraw.
    function withdraw(uint256 value) external;

    /// @notice Check the deposited balance, in Ether, of the given account.
    /// @param addr The address of the account.
    /// @return The balance of the account.
    function balanceOf(address addr) external view returns (uint256);

    /// @notice Withdraw funds from the market's treasury.
    /// @dev Value is debited from the market's account.
    /// @param value The amount to withdraw.
    function withdrawFromTreasury(uint256 value) external;

    /// @notice Withdraw funds from the market' stake treasury.
    /// @dev Value is debited from the market's account.
    /// @param value The amount to withdraw.
    function withdrawFromStakeTreasury(uint256 value) external;

    /// @notice Deposit stake into the market to pay for lockin stake.
    /// @dev Before calling this method, the account owner must approve the contract as an allowed spender.
    function depositStake(uint256 value) external;
    /// @notice Permit and deposit stake into the market to pay for lockin stake.
    /// @dev This method requires a valid EIP-712 signature from the account owner.
    function depositStakeWithPermit(uint256 value, uint256 deadline, uint8 v, bytes32 r, bytes32 s) external;
    /// @notice Withdraw stake from the market.
    function withdrawStake(uint256 value) external;
    /// @notice Check the deposited balance, in HP, of the given account.
    function balanceOfStake(address addr) external view returns (uint256);

    /// @notice Submit a request such that it is publicly available for provers to evaluate and bid on.
    /// Any `msg.value` sent with the call will be added to the balance of `msg.sender`.
    /// @dev Submitting the transaction only broadcasts it, and is not a required step.
    /// This method does not validate the signature or store any state related to the request.
    /// Verifying the signature here is not required for protocol safety as the signature is
    /// checked when the request is locked, and during fulfillment (by the assessor).
    /// @param request The proof request details.
    /// @param clientSignature The signature of the client.
    function submitRequest(ProofRequest calldata request, bytes calldata clientSignature) external payable;

    /// @notice Lock the request to the prover, giving them exclusive rights to be paid to
    /// fulfill this request, and also making them subject to slashing penalties if they fail to
    /// deliver. At this point, the price for fulfillment is also set, based on the reverse Dutch
    /// auction parameters and the block at which this transaction is processed.
    /// @dev This method should be called from the address of the prover.
    /// @param request The proof request details.
    /// @param clientSignature The signature of the client.
    function lockRequest(ProofRequest calldata request, bytes calldata clientSignature) external;

    /// @notice Lock the request to the prover, giving them exclusive rights to be paid to
    /// fulfill this request, and also making them subject to slashing penalties if they fail to
    /// deliver. At this point, the price for fulfillment is also set, based on the reverse Dutch
    /// auction parameters and the block at which this transaction is processed.
    /// @dev This method uses the provided signature to authenticate the prover.
    /// @param request The proof request details.
    /// @param clientSignature The signature of the client.
    /// @param proverSignature The signature of the prover.
    function lockRequestWithSignature(
        ProofRequest calldata request,
        bytes calldata clientSignature,
        bytes calldata proverSignature
    ) external;

    /// @notice Fulfill a request by delivering the proof for the application.
    /// If the order is locked, only the prover that locked the order may receive payment.
    /// If another prover delivers a proof for an order that is locked, this method will revert
    /// unless `paymentRequired` is set to `false` on the `Fulfillment` struct.
    /// @param fill The fulfillment information, including the journal and seal.
    /// @param assessorReceipt The Assessor's guest fulfillment information verified to confirm the
    /// request's requirements are met.
    function fulfill(Fulfillment calldata fill, AssessorReceipt calldata assessorReceipt)
        external
        returns (bytes memory paymentError);

    /// @notice Fulfills a batch of requests. See IBoundlessMarket.fulfill for more information.
    /// @param fills The array of fulfillment information.
    /// @param assessorReceipt The Assessor's guest fulfillment information verified to confirm the
    /// request's requirements are met.
    function fulfillBatch(Fulfillment[] calldata fills, AssessorReceipt calldata assessorReceipt)
        external
        returns (bytes[] memory paymentError);

    /// @notice Fulfill a request by delivering the proof for the application and withdraw from the prover balance.
    /// If the order is locked, only the prover that locked the order may receive payment.
    /// If another prover delivers a proof for an order that is locked, this method will revert
    /// unless `paymentRequired` is set to `false` on the `Fulfillment` struct.
    /// @param fill The fulfillment information, including the journal and seal.
    /// @param assessorReceipt The Assessor's guest fulfillment information verified to confirm the
    /// request's requirements are met.
    function fulfillAndWithdraw(Fulfillment calldata fill, AssessorReceipt calldata assessorReceipt)
        external
        returns (bytes memory paymentError);

    /// @notice Fulfills a batch of requests and withdraw from the prover balance. See IBoundlessMarket.fulfill for more information.
    /// @param fills The array of fulfillment information.
    /// @param assessorReceipt The Assessor's guest fulfillment information verified to confirm the
    /// request's requirements are met.
    function fulfillBatchAndWithdraw(Fulfillment[] calldata fills, AssessorReceipt calldata assessorReceipt)
        external
        returns (bytes[] memory paymentError);

    /// @notice Verify the application and assessor receipts, ensuring that the provided fulfillment
    /// satisfies the request.
    /// @param fill The fulfillment information, including the journal and seal.
    /// @param assessorReceipt The Assessor's guest fulfillment information verified to confirm the
    /// request's requirements are met.
    function verifyDelivery(Fulfillment calldata fill, AssessorReceipt calldata assessorReceipt) external view;

    /// @notice Verify the application and assessor receipts for the batch, ensuring that the provided
    /// fulfillments satisfy the requests.
    /// @param fills The array of fulfillment information.
    /// @param assessorReceipt The Assessor's guest fulfillment information verified to confirm the
    /// request's requirements are met.
    function verifyBatchDelivery(Fulfillment[] calldata fills, AssessorReceipt calldata assessorReceipt)
        external
        view;

    /// @notice Checks the validity of the request and then writes the current auction price to
    /// transient storage.
    /// @dev When called within the same transaction, this method can be used to fulfill a request
    /// that is not locked. This is useful when the prover wishes to fulfill a request, but does
    /// not want to issue a lock transaction e.g. because the stake is too high or to save money by
    /// avoiding the gas costs of the lock transaction.
    /// @param request The proof request details.
    /// @param clientSignature The signature of the client.
    function priceRequest(ProofRequest calldata request, bytes calldata clientSignature) external;

    /// @notice A combined call to `IBoundlessMarket.priceRequest` and `IBoundlessMarket.fulfill`.
    /// The caller should provide the signed request and signature for each unlocked request they
    /// want to fulfill. Payment for unlocked requests will go to the provided `prover` address.
    /// @param request The proof requests.
    /// @param clientSignature The client signatures.
    /// @param fill The fulfillment information.
    /// @param assessorReceipt The Assessor's guest fulfillment information verified to confirm the
    /// request's requirements are met.
    function priceAndFulfill(
        ProofRequest calldata request,
        bytes calldata clientSignature,
        Fulfillment calldata fill,
        AssessorReceipt calldata assessorReceipt
    ) external returns (bytes memory paymentError);

    /// @notice A combined call to `IBoundlessMarket.priceRequest` and `IBoundlessMarket.fulfillBatch`.
    /// The caller should provide the signed request and signature for each unlocked request they
    /// want to fulfill. Payment for unlocked requests will go to the provided `prover` address.
    /// @param requests The array of proof requests.
    /// @param clientSignatures The array of client signatures.
    /// @param fills The array of fulfillment information.
    /// @param assessorReceipt The Assessor's guest fulfillment information verified to confirm the
    /// request's requirements are met.
    function priceAndFulfillBatch(
        ProofRequest[] calldata requests,
        bytes[] calldata clientSignatures,
        Fulfillment[] calldata fills,
        AssessorReceipt calldata assessorReceipt
    ) external returns (bytes[] memory paymentError);

    /// @notice A combined call to `IBoundlessMarket.priceRequest` and `IBoundlessMarket.fulfillAndWithdraw`.
    /// The caller should provide the signed request and signature for each unlocked request they
    /// want to fulfill. Payment for unlocked requests will go to the provided `prover` address.
    /// @param request The proof requests.
    /// @param clientSignature The client signatures.
    /// @param fill The fulfillment information.
    /// @param assessorReceipt The Assessor's guest fulfillment information verified to confirm the
    /// request's requirements are met.
    function priceAndFulfillAndWithdraw(
        ProofRequest calldata request,
        bytes calldata clientSignature,
        Fulfillment calldata fill,
        AssessorReceipt calldata assessorReceipt
    ) external returns (bytes memory paymentError);

    /// @notice A combined call to `IBoundlessMarket.priceRequest` and `IBoundlessMarket.fulfillBatchAndWithdraw`.
    /// The caller should provide the signed request and signature for each unlocked request they
    /// want to fulfill. Payment for unlocked requests will go to the provided `prover` address.
    /// @param requests The array of proof requests.
    /// @param clientSignatures The array of client signatures.
    /// @param fills The array of fulfillment information.
    /// @param assessorReceipt The Assessor's guest fulfillment information verified to confirm the
    /// request's requirements are met.
    function priceAndFulfillBatchAndWithdraw(
        ProofRequest[] calldata requests,
        bytes[] calldata clientSignatures,
        Fulfillment[] calldata fills,
        AssessorReceipt calldata assessorReceipt
    ) external returns (bytes[] memory paymentError);

    /// @notice Submit a new root to a set-verifier.
    /// @dev Consider using `submitRootAndFulfillBatch` to submit the root and fulfill in one transaction.
    /// @param setVerifier The address of the set-verifier contract.
    /// @param root The new merkle root.
    /// @param seal The seal of the new merkle root.
    function submitRoot(address setVerifier, bytes32 root, bytes calldata seal) external;

    /// @notice Combined function to submit a new root to a set-verifier and call fulfillBatch.
    /// @dev Useful to reduce the transaction count for fulfillments.
    /// @param setVerifier The address of the set-verifier contract.
    /// @param root The new merkle root.
    /// @param seal The seal of the new merkle root.
    /// @param fills The array of fulfillment information.
    /// @param assessorReceipt The Assessor's guest fulfillment information verified to confirm the
    /// request's requirements are met.
    function submitRootAndFulfillBatch(
        address setVerifier,
        bytes32 root,
        bytes calldata seal,
        Fulfillment[] calldata fills,
        AssessorReceipt calldata assessorReceipt
    ) external returns (bytes[] memory paymentError);

    /// @notice Combined function to submit a new root to a set-verifier and call fulfillBatchAndWithdraw.
    /// @dev Useful to reduce the transaction count for fulfillments.
    /// @param setVerifier The address of the set-verifier contract.
    /// @param root The new merkle root.
    /// @param seal The seal of the new merkle root.
    /// @param fills The array of fulfillment information.
    /// @param assessorReceipt The Assessor's guest fulfillment information verified to confirm the
    /// request's requirements are met.
    function submitRootAndFulfillBatchAndWithdraw(
        address setVerifier,
        bytes32 root,
        bytes calldata seal,
        Fulfillment[] calldata fills,
        AssessorReceipt calldata assessorReceipt
    ) external returns (bytes[] memory paymentError);

    /// @notice When a prover fails to fulfill a request by the deadline, this method can be used to burn
    /// the associated prover stake.
    /// @dev The provers stake has already been transferred to the contract when the request was locked.
    ///      This method just burn the stake.
    /// @param requestId The ID of the request.
    function slash(RequestId requestId) external;

    /// @notice EIP 712 domain separator getter.
    /// @return The EIP 712 domain separator.
    function eip712DomainSeparator() external view returns (bytes32);

    /// @notice Returns the assessor imageId and its url.
    /// @return The imageId and its url.
    function imageInfo() external view returns (bytes32, string memory);

    /// Returns the address of the token used for stake deposits.
    // solhint-disable-next-line func-name-mixedcase
    function STAKE_TOKEN_CONTRACT() external view returns (address);
}
