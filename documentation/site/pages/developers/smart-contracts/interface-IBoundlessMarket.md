# IBoundlessMarket

## Functions

### requestIsLocked

Check if the given request has been locked (i.e. accepted) by a prover.

_When a request is locked, only the prover it is locked to can be paid to fulfill the job._

```solidity
function requestIsLocked(RequestId requestId) external view returns (bool);
```

**Parameters**

| Name        | Type        | Description            |
| ----------- | ----------- | ---------------------- |
| `requestId` | `RequestId` | The ID of the request. |

**Returns**

| Name     | Type   | Description                                     |
| -------- | ------ | ----------------------------------------------- |
| `<none>` | `bool` | True if the request is locked, false otherwise. |

### requestIsSlashed

Check if the given request resulted in the prover being slashed
(i.e. request was locked in but proof was not delivered)

_Note it is possible for a request to result in a slash, but still be fulfilled
if for example another prover decided to fulfill the request altruistically.
This function should not be used to determine if a request was fulfilled._

```solidity
function requestIsSlashed(RequestId requestId) external view returns (bool);
```

**Parameters**

| Name        | Type        | Description            |
| ----------- | ----------- | ---------------------- |
| `requestId` | `RequestId` | The ID of the request. |

**Returns**

| Name     | Type   | Description                                                                |
| -------- | ------ | -------------------------------------------------------------------------- |
| `<none>` | `bool` | True if the request resulted in the prover being slashed, false otherwise. |

### requestIsFulfilled

Check if the given request has been fulfilled (i.e. a proof was delivered).

```solidity
function requestIsFulfilled(RequestId requestId) external view returns (bool);
```

**Parameters**

| Name        | Type        | Description            |
| ----------- | ----------- | ---------------------- |
| `requestId` | `RequestId` | The ID of the request. |

**Returns**

| Name     | Type   | Description                                        |
| -------- | ------ | -------------------------------------------------- |
| `<none>` | `bool` | True if the request is fulfilled, false otherwise. |

### requestLockDeadline

For a given locked request, returns when the lock expires.

_If the request is not locked, this function will revert._

```solidity
function requestLockDeadline(RequestId requestId) external view returns (uint64);
```

**Parameters**

| Name        | Type        | Description            |
| ----------- | ----------- | ---------------------- |
| `requestId` | `RequestId` | The ID of the request. |

**Returns**

| Name     | Type     | Description                                     |
| -------- | -------- | ----------------------------------------------- |
| `<none>` | `uint64` | The expiration time of the lock on the request. |

### requestDeadline

For a given locked request, returns when request expires.

_If the request is not locked, this function will revert._

```solidity
function requestDeadline(RequestId requestId) external view returns (uint64);
```

**Parameters**

| Name        | Type        | Description            |
| ----------- | ----------- | ---------------------- |
| `requestId` | `RequestId` | The ID of the request. |

**Returns**

| Name     | Type     | Description                         |
| -------- | -------- | ----------------------------------- |
| `<none>` | `uint64` | The expiration time of the request. |

### deposit

Deposit Ether into the market to pay for proof.

_Value deposited is msg.value and it is credited to the account of msg.sender._

```solidity
function deposit() external payable;
```

### withdraw

Withdraw Ether from the market.

_Value is debited from msg.sender._

```solidity
function withdraw(uint256 value) external;
```

**Parameters**

| Name    | Type      | Description             |
| ------- | --------- | ----------------------- |
| `value` | `uint256` | The amount to withdraw. |

### balanceOf

Check the deposited balance, in Ether, of the given account.

```solidity
function balanceOf(address addr) external view returns (uint256);
```

**Parameters**

| Name   | Type      | Description                 |
| ------ | --------- | --------------------------- |
| `addr` | `address` | The address of the account. |

**Returns**

| Name     | Type      | Description                 |
| -------- | --------- | --------------------------- |
| `<none>` | `uint256` | The balance of the account. |

### withdrawFromTreasury

Withdraw funds from the market's treasury.

_Value is debited from the market's account._

```solidity
function withdrawFromTreasury(uint256 value) external;
```

**Parameters**

| Name    | Type      | Description             |
| ------- | --------- | ----------------------- |
| `value` | `uint256` | The amount to withdraw. |

### withdrawFromStakeTreasury

Withdraw funds from the market' stake treasury.

_Value is debited from the market's account._

```solidity
function withdrawFromStakeTreasury(uint256 value) external;
```

**Parameters**

| Name    | Type      | Description             |
| ------- | --------- | ----------------------- |
| `value` | `uint256` | The amount to withdraw. |

### depositStake

Deposit stake into the market to pay for lockin stake.

_Before calling this method, the account owner must approve the contract as an allowed spender._

```solidity
function depositStake(uint256 value) external;
```

### depositStakeWithPermit

Permit and deposit stake into the market to pay for lockin stake.

_This method requires a valid EIP-712 signature from the account owner._

```solidity
function depositStakeWithPermit(uint256 value, uint256 deadline, uint8 v, bytes32 r, bytes32 s) external;
```

### withdrawStake

Withdraw stake from the market.

```solidity
function withdrawStake(uint256 value) external;
```

### balanceOfStake

Check the deposited balance, in HP, of the given account.

```solidity
function balanceOfStake(address addr) external view returns (uint256);
```

### submitRequest

Submit a request such that it is publicly available for provers to evaluate and bid on.
Any `msg.value` sent with the call will be added to the balance of `msg.sender`.

_Submitting the transaction only broadcasts it, and is not a required step.
This method does not validate the signature or store any state related to the request.
Verifying the signature here is not required for protocol safety as the signature is
checked when the request is locked, and during fulfillment (by the assessor)._

```solidity
function submitRequest(ProofRequest calldata request, bytes calldata clientSignature) external payable;
```

**Parameters**

| Name              | Type           | Description                  |
| ----------------- | -------------- | ---------------------------- |
| `request`         | `ProofRequest` | The proof request details.   |
| `clientSignature` | `bytes`        | The signature of the client. |

### lockRequest

Lock the request to the prover, giving them exclusive rights to be paid to
fulfill this request, and also making them subject to slashing penalties if they fail to
deliver. At this point, the price for fulfillment is also set, based on the reverse Dutch
auction parameters and the time at which this transaction is processed.

_This method should be called from the address of the prover._

```solidity
function lockRequest(ProofRequest calldata request, bytes calldata clientSignature) external;
```

**Parameters**

| Name              | Type           | Description                  |
| ----------------- | -------------- | ---------------------------- |
| `request`         | `ProofRequest` | The proof request details.   |
| `clientSignature` | `bytes`        | The signature of the client. |

### lockRequestWithSignature

Lock the request to the prover, giving them exclusive rights to be paid to
fulfill this request, and also making them subject to slashing penalties if they fail to
deliver. At this point, the price for fulfillment is also set, based on the reverse Dutch
auction parameters and the time at which this transaction is processed.

_This method uses the provided signature to authenticate the prover._

```solidity
function lockRequestWithSignature(
    ProofRequest calldata request,
    bytes calldata clientSignature,
    bytes calldata proverSignature
) external;
```

**Parameters**

| Name              | Type           | Description                  |
| ----------------- | -------------- | ---------------------------- |
| `request`         | `ProofRequest` | The proof request details.   |
| `clientSignature` | `bytes`        | The signature of the client. |
| `proverSignature` | `bytes`        | The signature of the prover. |

### fulfill

Fulfill a request by delivering the proof for the application.
If the order is locked, only the prover that locked the order may receive payment.
If another prover delivers a proof for an order that is locked, this method will revert
unless `paymentRequired` is set to `false` on the `Fulfillment` struct.

```solidity
function fulfill(Fulfillment calldata fill, AssessorReceipt calldata assessorReceipt)
    external
    returns (bytes memory paymentError);
```

**Parameters**

| Name              | Type              | Description                                                                                          |
| ----------------- | ----------------- | ---------------------------------------------------------------------------------------------------- |
| `fill`            | `Fulfillment`     | The fulfillment information, including the journal and seal.                                         |
| `assessorReceipt` | `AssessorReceipt` | The Assessor's guest fulfillment information verified to confirm the request's requirements are met. |

### fulfillBatch

Fulfills a batch of requests. See IBoundlessMarket.fulfill for more information.

```solidity
function fulfillBatch(Fulfillment[] calldata fills, AssessorReceipt calldata assessorReceipt)
    external
    returns (bytes[] memory paymentError);
```

**Parameters**

| Name              | Type              | Description                                                                                          |
| ----------------- | ----------------- | ---------------------------------------------------------------------------------------------------- |
| `fills`           | `Fulfillment[]`   | The array of fulfillment information.                                                                |
| `assessorReceipt` | `AssessorReceipt` | The Assessor's guest fulfillment information verified to confirm the request's requirements are met. |

### fulfillAndWithdraw

Fulfill a request by delivering the proof for the application and withdraw from the prover balance.
If the order is locked, only the prover that locked the order may receive payment.
If another prover delivers a proof for an order that is locked, this method will revert
unless `paymentRequired` is set to `false` on the `Fulfillment` struct.

```solidity
function fulfillAndWithdraw(Fulfillment calldata fill, AssessorReceipt calldata assessorReceipt)
    external
    returns (bytes memory paymentError);
```

**Parameters**

| Name              | Type              | Description                                                                                          |
| ----------------- | ----------------- | ---------------------------------------------------------------------------------------------------- |
| `fill`            | `Fulfillment`     | The fulfillment information, including the journal and seal.                                         |
| `assessorReceipt` | `AssessorReceipt` | The Assessor's guest fulfillment information verified to confirm the request's requirements are met. |

### fulfillBatchAndWithdraw

Fulfills a batch of requests and withdraw from the prover balance. See IBoundlessMarket.fulfill for more information.

```solidity
function fulfillBatchAndWithdraw(Fulfillment[] calldata fills, AssessorReceipt calldata assessorReceipt)
    external
    returns (bytes[] memory paymentError);
```

**Parameters**

| Name              | Type              | Description                                                                                          |
| ----------------- | ----------------- | ---------------------------------------------------------------------------------------------------- |
| `fills`           | `Fulfillment[]`   | The array of fulfillment information.                                                                |
| `assessorReceipt` | `AssessorReceipt` | The Assessor's guest fulfillment information verified to confirm the request's requirements are met. |

### verifyDelivery

Verify the application and assessor receipts, ensuring that the provided fulfillment
satisfies the request.

```solidity
function verifyDelivery(Fulfillment calldata fill, AssessorReceipt calldata assessorReceipt) external view;
```

**Parameters**

| Name              | Type              | Description                                                                                          |
| ----------------- | ----------------- | ---------------------------------------------------------------------------------------------------- |
| `fill`            | `Fulfillment`     | The fulfillment information, including the journal and seal.                                         |
| `assessorReceipt` | `AssessorReceipt` | The Assessor's guest fulfillment information verified to confirm the request's requirements are met. |

### verifyBatchDelivery

Verify the application and assessor receipts for the batch, ensuring that the provided
fulfillments satisfy the requests.

```solidity
function verifyBatchDelivery(Fulfillment[] calldata fills, AssessorReceipt calldata assessorReceipt) external view;
```

**Parameters**

| Name              | Type              | Description                                                                                          |
| ----------------- | ----------------- | ---------------------------------------------------------------------------------------------------- |
| `fills`           | `Fulfillment[]`   | The array of fulfillment information.                                                                |
| `assessorReceipt` | `AssessorReceipt` | The Assessor's guest fulfillment information verified to confirm the request's requirements are met. |

### priceRequest

Checks the validity of the request and then writes the current auction price to
transient storage.

_When called within the same transaction, this method can be used to fulfill a request
that is not locked. This is useful when the prover wishes to fulfill a request, but does
not want to issue a lock transaction e.g. because the stake is too high or to save money by
avoiding the gas costs of the lock transaction._

```solidity
function priceRequest(ProofRequest calldata request, bytes calldata clientSignature) external;
```

**Parameters**

| Name              | Type           | Description                  |
| ----------------- | -------------- | ---------------------------- |
| `request`         | `ProofRequest` | The proof request details.   |
| `clientSignature` | `bytes`        | The signature of the client. |

### priceAndFulfill

A combined call to `IBoundlessMarket.priceRequest` and `IBoundlessMarket.fulfill`.
The caller should provide the signed request and signature for each unlocked request they
want to fulfill. Payment for unlocked requests will go to the provided `prover` address.

```solidity
function priceAndFulfill(
    ProofRequest calldata request,
    bytes calldata clientSignature,
    Fulfillment calldata fill,
    AssessorReceipt calldata assessorReceipt
) external returns (bytes memory paymentError);
```

**Parameters**

| Name              | Type              | Description                                                                                          |
| ----------------- | ----------------- | ---------------------------------------------------------------------------------------------------- |
| `request`         | `ProofRequest`    | The proof requests.                                                                                  |
| `clientSignature` | `bytes`           | The client signatures.                                                                               |
| `fill`            | `Fulfillment`     | The fulfillment information.                                                                         |
| `assessorReceipt` | `AssessorReceipt` | The Assessor's guest fulfillment information verified to confirm the request's requirements are met. |

### priceAndFulfillBatch

A combined call to `IBoundlessMarket.priceRequest` and `IBoundlessMarket.fulfillBatch`.
The caller should provide the signed request and signature for each unlocked request they
want to fulfill. Payment for unlocked requests will go to the provided `prover` address.

```solidity
function priceAndFulfillBatch(
    ProofRequest[] calldata requests,
    bytes[] calldata clientSignatures,
    Fulfillment[] calldata fills,
    AssessorReceipt calldata assessorReceipt
) external returns (bytes[] memory paymentError);
```

**Parameters**

| Name               | Type              | Description                                                                                          |
| ------------------ | ----------------- | ---------------------------------------------------------------------------------------------------- |
| `requests`         | `ProofRequest[]`  | The array of proof requests.                                                                         |
| `clientSignatures` | `bytes[]`         | The array of client signatures.                                                                      |
| `fills`            | `Fulfillment[]`   | The array of fulfillment information.                                                                |
| `assessorReceipt`  | `AssessorReceipt` | The Assessor's guest fulfillment information verified to confirm the request's requirements are met. |

### priceAndFulfillAndWithdraw

A combined call to `IBoundlessMarket.priceRequest` and `IBoundlessMarket.fulfillAndWithdraw`.
The caller should provide the signed request and signature for each unlocked request they
want to fulfill. Payment for unlocked requests will go to the provided `prover` address.

```solidity
function priceAndFulfillAndWithdraw(
    ProofRequest calldata request,
    bytes calldata clientSignature,
    Fulfillment calldata fill,
    AssessorReceipt calldata assessorReceipt
) external returns (bytes memory paymentError);
```

**Parameters**

| Name              | Type              | Description                                                                                          |
| ----------------- | ----------------- | ---------------------------------------------------------------------------------------------------- |
| `request`         | `ProofRequest`    | The proof requests.                                                                                  |
| `clientSignature` | `bytes`           | The client signatures.                                                                               |
| `fill`            | `Fulfillment`     | The fulfillment information.                                                                         |
| `assessorReceipt` | `AssessorReceipt` | The Assessor's guest fulfillment information verified to confirm the request's requirements are met. |

### priceAndFulfillBatchAndWithdraw

A combined call to `IBoundlessMarket.priceRequest` and `IBoundlessMarket.fulfillBatchAndWithdraw`.
The caller should provide the signed request and signature for each unlocked request they
want to fulfill. Payment for unlocked requests will go to the provided `prover` address.

```solidity
function priceAndFulfillBatchAndWithdraw(
    ProofRequest[] calldata requests,
    bytes[] calldata clientSignatures,
    Fulfillment[] calldata fills,
    AssessorReceipt calldata assessorReceipt
) external returns (bytes[] memory paymentError);
```

**Parameters**

| Name               | Type              | Description                                                                                          |
| ------------------ | ----------------- | ---------------------------------------------------------------------------------------------------- |
| `requests`         | `ProofRequest[]`  | The array of proof requests.                                                                         |
| `clientSignatures` | `bytes[]`         | The array of client signatures.                                                                      |
| `fills`            | `Fulfillment[]`   | The array of fulfillment information.                                                                |
| `assessorReceipt`  | `AssessorReceipt` | The Assessor's guest fulfillment information verified to confirm the request's requirements are met. |

### submitRoot

Submit a new root to a set-verifier.

_Consider using `submitRootAndFulfillBatch` to submit the root and fulfill in one transaction._

```solidity
function submitRoot(address setVerifier, bytes32 root, bytes calldata seal) external;
```

**Parameters**

| Name          | Type      | Description                               |
| ------------- | --------- | ----------------------------------------- |
| `setVerifier` | `address` | The address of the set-verifier contract. |
| `root`        | `bytes32` | The new merkle root.                      |
| `seal`        | `bytes`   | The seal of the new merkle root.          |

### submitRootAndFulfillBatch

Combined function to submit a new root to a set-verifier and call fulfillBatch.

_Useful to reduce the transaction count for fulfillments._

```solidity
function submitRootAndFulfillBatch(
    address setVerifier,
    bytes32 root,
    bytes calldata seal,
    Fulfillment[] calldata fills,
    AssessorReceipt calldata assessorReceipt
) external returns (bytes[] memory paymentError);
```

**Parameters**

| Name              | Type              | Description                                                                                          |
| ----------------- | ----------------- | ---------------------------------------------------------------------------------------------------- |
| `setVerifier`     | `address`         | The address of the set-verifier contract.                                                            |
| `root`            | `bytes32`         | The new merkle root.                                                                                 |
| `seal`            | `bytes`           | The seal of the new merkle root.                                                                     |
| `fills`           | `Fulfillment[]`   | The array of fulfillment information.                                                                |
| `assessorReceipt` | `AssessorReceipt` | The Assessor's guest fulfillment information verified to confirm the request's requirements are met. |

### submitRootAndFulfillBatchAndWithdraw

Combined function to submit a new root to a set-verifier and call fulfillBatchAndWithdraw.

_Useful to reduce the transaction count for fulfillments._

```solidity
function submitRootAndFulfillBatchAndWithdraw(
    address setVerifier,
    bytes32 root,
    bytes calldata seal,
    Fulfillment[] calldata fills,
    AssessorReceipt calldata assessorReceipt
) external returns (bytes[] memory paymentError);
```

**Parameters**

| Name              | Type              | Description                                                                                          |
| ----------------- | ----------------- | ---------------------------------------------------------------------------------------------------- |
| `setVerifier`     | `address`         | The address of the set-verifier contract.                                                            |
| `root`            | `bytes32`         | The new merkle root.                                                                                 |
| `seal`            | `bytes`           | The seal of the new merkle root.                                                                     |
| `fills`           | `Fulfillment[]`   | The array of fulfillment information.                                                                |
| `assessorReceipt` | `AssessorReceipt` | The Assessor's guest fulfillment information verified to confirm the request's requirements are met. |

### slash

When a prover fails to fulfill a request by the deadline, this method can be used to burn
the associated prover stake.

_The provers stake has already been transferred to the contract when the request was locked.
This method just burn the stake._

```solidity
function slash(RequestId requestId) external;
```

**Parameters**

| Name        | Type        | Description            |
| ----------- | ----------- | ---------------------- |
| `requestId` | `RequestId` | The ID of the request. |

### eip712DomainSeparator

EIP 712 domain separator getter.

```solidity
function eip712DomainSeparator() external view returns (bytes32);
```

**Returns**

| Name     | Type      | Description                   |
| -------- | --------- | ----------------------------- |
| `<none>` | `bytes32` | The EIP 712 domain separator. |

### imageInfo

Returns the assessor imageId and its url.

```solidity
function imageInfo() external view returns (bytes32, string memory);
```

**Returns**

| Name     | Type      | Description              |
| -------- | --------- | ------------------------ |
| `<none>` | `bytes32` | The imageId and its url. |
| `<none>` | `string`  |                          |

### STAKE_TOKEN_CONTRACT

Returns the address of the token used for stake deposits.

```solidity
function STAKE_TOKEN_CONTRACT() external view returns (address);
```

## Events

### RequestSubmitted

Event logged when a new proof request is submitted by a client.

_Note that the signature is not verified by the contract and should instead be verified
by the receiver of the event._

```solidity
event RequestSubmitted(RequestId indexed requestId);
```

**Parameters**

| Name        | Type        | Description            |
| ----------- | ----------- | ---------------------- |
| `requestId` | `RequestId` | The ID of the request. |

### RequestLocked

Event logged when a request is locked in by the given prover.

```solidity
event RequestLocked(RequestId indexed requestId, address prover);
```

**Parameters**

| Name        | Type        | Description                |
| ----------- | ----------- | -------------------------- |
| `requestId` | `RequestId` | The ID of the request.     |
| `prover`    | `address`   | The address of the prover. |

### RequestFulfilled

Event logged when a request is fulfilled.

```solidity
event RequestFulfilled(RequestId indexed requestId);
```

**Parameters**

| Name        | Type        | Description            |
| ----------- | ----------- | ---------------------- |
| `requestId` | `RequestId` | The ID of the request. |

### ProofDelivered

Event logged when a proof is delivered that satisfies the request's requirements.

_It is possible for this event to be logged multiple times for a single request. This
is usually logged as part of order fulfillment, however it can also be logged by a prover
sending the proof without payment._

```solidity
event ProofDelivered(RequestId indexed requestId);
```

**Parameters**

| Name        | Type        | Description            |
| ----------- | ----------- | ---------------------- |
| `requestId` | `RequestId` | The ID of the request. |

### ProverSlashed

Event when a prover is slashed is made to the market.

```solidity
event ProverSlashed(RequestId indexed requestId, uint256 stakeBurned, uint256 stakeTransferred, address stakeRecipient);
```

**Parameters**

| Name               | Type        | Description                                                                                 |
| ------------------ | ----------- | ------------------------------------------------------------------------------------------- |
| `requestId`        | `RequestId` | The ID of the request.                                                                      |
| `stakeBurned`      | `uint256`   | The amount of stake burned.                                                                 |
| `stakeTransferred` | `uint256`   | The amount of stake transferred to either the fulfilling prover or the market.              |
| `stakeRecipient`   | `address`   | The address of the stake recipient. Typically the fulfilling prover, but can be the market. |

### Deposit

Event when a deposit is made to the market.

```solidity
event Deposit(address indexed account, uint256 value);
```

**Parameters**

| Name      | Type      | Description                     |
| --------- | --------- | ------------------------------- |
| `account` | `address` | The account making the deposit. |
| `value`   | `uint256` | The value of the deposit.       |

### Withdrawal

Event when a withdrawal is made from the market.

```solidity
event Withdrawal(address indexed account, uint256 value);
```

**Parameters**

| Name      | Type      | Description                        |
| --------- | --------- | ---------------------------------- |
| `account` | `address` | The account making the withdrawal. |
| `value`   | `uint256` | The value of the withdrawal.       |

### StakeDeposit

Event when a stake deposit is made to the market.

```solidity
event StakeDeposit(address indexed account, uint256 value);
```

**Parameters**

| Name      | Type      | Description                     |
| --------- | --------- | ------------------------------- |
| `account` | `address` | The account making the deposit. |
| `value`   | `uint256` | The value of the deposit.       |

### StakeWithdrawal

Event when a stake withdrawal is made to the market.

```solidity
event StakeWithdrawal(address indexed account, uint256 value);
```

**Parameters**

| Name      | Type      | Description                        |
| --------- | --------- | ---------------------------------- |
| `account` | `address` | The account making the withdrawal. |
| `value`   | `uint256` | The value of the withdrawal.       |

### Upgraded

Event when the contract is upgraded to a new version.

```solidity
event Upgraded(uint64 indexed version);
```

**Parameters**

| Name      | Type     | Description                      |
| --------- | -------- | -------------------------------- |
| `version` | `uint64` | The new version of the contract. |

### PaymentRequirementsFailed

Event emitted during fulfillment if a request was fulfilled, but payment was not
transferred because at least one condition was not met. See the documentation on
`IBoundlessMarket.fulfillBatch` for more information.

_The payload of the event is an ABI encoded error, from the errors on this contract.
If there is an unexpired lock on the request, the order, the prover holding the lock may
still be able to receive payment by sending another transaction._

```solidity
event PaymentRequirementsFailed(bytes error);
```

**Parameters**

| Name    | Type    | Description            |
| ------- | ------- | ---------------------- |
| `error` | `bytes` | The ABI encoded error. |

### CallbackFailed

Event emitted when a callback to a contract fails during fulfillment

```solidity
event CallbackFailed(RequestId indexed requestId, address callback, bytes error);
```

**Parameters**

| Name        | Type        | Description                                      |
| ----------- | ----------- | ------------------------------------------------ |
| `requestId` | `RequestId` | The ID of the request that was being fulfilled   |
| `callback`  | `address`   | The address of the callback contract that failed |
| `error`     | `bytes`     | The error message from the failed call           |

## Errors

### RequestIsLocked

Error when a request is locked when it was not required to be.

```solidity
error RequestIsLocked(RequestId requestId);
```

**Parameters**

| Name        | Type        | Description            |
| ----------- | ----------- | ---------------------- |
| `requestId` | `RequestId` | The ID of the request. |

### RequestIsExpiredOrNotPriced

Error when a request is expired or not priced when it was required to be.
Either locking the request, or calling the `IBoundlessMarket.priceRequest` function
in the same transaction will satisfy this requirement.

```solidity
error RequestIsExpiredOrNotPriced(RequestId requestId);
```

**Parameters**

| Name        | Type        | Description            |
| ----------- | ----------- | ---------------------- |
| `requestId` | `RequestId` | The ID of the request. |

### RequestIsNotLocked

Error when a request is not locked when it was required to be.

```solidity
error RequestIsNotLocked(RequestId requestId);
```

**Parameters**

| Name        | Type        | Description            |
| ----------- | ----------- | ---------------------- |
| `requestId` | `RequestId` | The ID of the request. |

### RequestIsFulfilled

Error when a request is fulfilled when it was not required to be.

```solidity
error RequestIsFulfilled(RequestId requestId);
```

**Parameters**

| Name        | Type        | Description            |
| ----------- | ----------- | ---------------------- |
| `requestId` | `RequestId` | The ID of the request. |

### RequestIsSlashed

Error when a request is slashed when it was not required to be.

```solidity
error RequestIsSlashed(RequestId requestId);
```

**Parameters**

| Name        | Type        | Description            |
| ----------- | ----------- | ---------------------- |
| `requestId` | `RequestId` | The ID of the request. |

### RequestLockIsExpired

Error when a request lock is no longer valid, as the lock deadline has passed.

```solidity
error RequestLockIsExpired(RequestId requestId, uint64 lockDeadline);
```

**Parameters**

| Name           | Type        | Description                       |
| -------------- | ----------- | --------------------------------- |
| `requestId`    | `RequestId` | The ID of the request.            |
| `lockDeadline` | `uint64`    | The lock deadline of the request. |

### RequestIsExpired

Error when a request is no longer valid, as the deadline has passed.

```solidity
error RequestIsExpired(RequestId requestId, uint64 deadline);
```

**Parameters**

| Name        | Type        | Description                  |
| ----------- | ----------- | ---------------------------- |
| `requestId` | `RequestId` | The ID of the request.       |
| `deadline`  | `uint64`    | The deadline of the request. |

### RequestIsNotExpired

Error when a request is still valid, as the deadline has yet to pass.

```solidity
error RequestIsNotExpired(RequestId requestId, uint64 deadline);
```

**Parameters**

| Name        | Type        | Description                  |
| ----------- | ----------- | ---------------------------- |
| `requestId` | `RequestId` | The ID of the request.       |
| `deadline`  | `uint64`    | The deadline of the request. |

### InvalidRequestFulfillment

Error when request being fulfilled doesn't match the request that was locked.

_This can happen if a client signs multiple requests with the same ID (i.e. multiple
versions of the same request) and a prover locks one version but then tries to call fulfill
using a different version._

```solidity
error InvalidRequestFulfillment(RequestId requestId, bytes32 provided, bytes32 locked);
```

**Parameters**

| Name        | Type        | Description               |
| ----------- | ----------- | ------------------------- |
| `requestId` | `RequestId` | The ID of the request.    |
| `provided`  | `bytes32`   | The provided fingerprint. |
| `locked`    | `bytes32`   | The locked fingerprint.   |

### InsufficientBalance

Error when unable to complete request because of insufficient balance.

```solidity
error InsufficientBalance(address account);
```

**Parameters**

| Name      | Type      | Description                            |
| --------- | --------- | -------------------------------------- |
| `account` | `address` | The account with insufficient balance. |

### InvalidSignature

Error when a signature did not pass verification checks.

```solidity
error InvalidSignature();
```

### InvalidRequest

Error when a request is malformed or internally inconsistent.

```solidity
error InvalidRequest();
```

### TransferFailed

Error when transfer of funds to an external address fails.

```solidity
error TransferFailed();
```

### SelectorMismatch

Error when providing a seal with a different selector than required.

```solidity
error SelectorMismatch(bytes4 required, bytes4 provided);
```

### BatchSizeExceedsLimit

Error when the batch size exceeds the limit.

```solidity
error BatchSizeExceedsLimit(uint256 batchSize, uint256 limit);
```
