# BoundlessMarket

**Inherits:**
[IBoundlessMarket](/developers/smart-contracts/interface-IBoundlessMarket), Initializable, EIP712Upgradeable, Ownable2StepUpgradeable, UUPSUpgradeable

## State Variables

### VERSION

_The version of the contract, with respect to upgrades._

```solidity
uint64 public constant VERSION = 1;
```

### requestLocks

Mapping of request ID to lock-in state. Non-zero for requests that are locked in.

```solidity
mapping(RequestId => RequestLock) public requestLocks;
```

### accounts

Mapping of address to account state.

```solidity
mapping(address => Account) internal accounts;
```

### VERIFIER

**Note:**
oz-upgrades-unsafe-allow: state-variable-immutable

```solidity
IRiscZeroVerifier public immutable VERIFIER;
```

### ASSESSOR_ID

**Note:**
oz-upgrades-unsafe-allow: state-variable-immutable

```solidity
bytes32 public immutable ASSESSOR_ID;
```

### imageUrl

```solidity
string private imageUrl;
```

### STAKE_TOKEN_CONTRACT

**Note:**
oz-upgrades-unsafe-allow: state-variable-immutable

```solidity
address public immutable STAKE_TOKEN_CONTRACT;
```

### DEFAULT_MAX_GAS_FOR_VERIFY

If no selector is specified as part of the request's requirements, the prover must provide
a proof that can be verified with at most the amount of gas specified by this constant.
This requirement exists to ensure that by default, the client can then post the given proof
in a new transaction as part of the application.

```solidity
uint256 public constant DEFAULT_MAX_GAS_FOR_VERIFY = 50000;
```

### SLASHING_BURN_BPS

When a prover is slashed for failing to fulfill a request, a portion of the stake
is burned, and the remaining portion is either send to the prover that ultimately fulfilled
the order, or to the market treasury. This fraction controls that ratio.

_The fee is configured as a constant to avoid accessing storage and thus paying for the
gas of an SLOAD. Can only be changed via contract upgrade._

```solidity
uint256 public constant SLASHING_BURN_BPS = 7500;
```

### MARKET_FEE_BPS

When an order is fulfilled, the market takes a fee based on the price of the order.
This fraction is multiplied by the price to decide the fee.

_The fee is configured as a constant to avoid accessing storage and thus paying for the
gas of an SLOAD. Can only be changed via contract upgrade._

```solidity
uint96 public constant MARKET_FEE_BPS = 0;
```

## Functions

### constructor

**Note:**
oz-upgrades-unsafe-allow: constructor

```solidity
constructor(IRiscZeroVerifier verifier, bytes32 assessorId, address stakeTokenContract);
```

### initialize

```solidity
function initialize(address initialOwner, string calldata _imageUrl) external initializer;
```

### setImageUrl

```solidity
function setImageUrl(string calldata _imageUrl) external onlyOwner;
```

### _authorizeUpgrade

```solidity
function _authorizeUpgrade(address newImplementation) internal override onlyOwner;
```

### submitRequest

```solidity
function submitRequest(ProofRequest calldata request, bytes calldata clientSignature) external payable;
```

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

### _lockRequest

Locks the request to the prover. Deducts funds from the client for payment
and funding from the prover for locking stake.

```solidity
function _lockRequest(
    ProofRequest calldata request,
    bytes32 requestDigest,
    address client,
    uint32 idx,
    address prover,
    uint64 lockDeadline,
    uint64 deadline
) internal;
```

### priceRequest

Validates the request and records the price to transient storage such that it can be
fulfilled within the same transaction without taking a lock on it.

_When called within the same transaction, this method can be used to fulfill a request
that is not locked. This is useful when the prover wishes to fulfill a request, but does
not want to issue a lock transaction e.g. because the stake is too high or to save money by
avoiding the gas costs of the lock transaction._

```solidity
function priceRequest(ProofRequest calldata request, bytes calldata clientSignature) public;
```

**Parameters**

| Name              | Type           | Description                  |
| ----------------- | -------------- | ---------------------------- |
| `request`         | `ProofRequest` | The proof request details.   |
| `clientSignature` | `bytes`        | The signature of the client. |

### verifyDelivery

Verify the application and assessor receipts, ensuring that the provided fulfillment
satisfies the request.

```solidity
function verifyDelivery(Fulfillment calldata fill, AssessorReceipt calldata assessorReceipt) public view;
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
function verifyBatchDelivery(Fulfillment[] calldata fills, AssessorReceipt calldata assessorReceipt) public view;
```

**Parameters**

| Name              | Type              | Description                                                                                          |
| ----------------- | ----------------- | ---------------------------------------------------------------------------------------------------- |
| `fills`           | `Fulfillment[]`   | The array of fulfillment information.                                                                |
| `assessorReceipt` | `AssessorReceipt` | The Assessor's guest fulfillment information verified to confirm the request's requirements are met. |

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

### fulfill

Fulfill a request by delivering the proof for the application.
If the order is locked, only the prover that locked the order may receive payment.
If another prover delivers a proof for an order that is locked, this method will revert
unless `paymentRequired` is set to `false` on the `Fulfillment` struct.

```solidity
function fulfill(Fulfillment calldata fill, AssessorReceipt calldata assessorReceipt)
    public
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
    public
    returns (bytes[] memory paymentError);
```

**Parameters**

| Name              | Type              | Description                                                                                          |
| ----------------- | ----------------- | ---------------------------------------------------------------------------------------------------- |
| `fills`           | `Fulfillment[]`   | The array of fulfillment information.                                                                |
| `assessorReceipt` | `AssessorReceipt` | The Assessor's guest fulfillment information verified to confirm the request's requirements are met. |

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

### fulfillAndWithdraw

Fulfill a request by delivering the proof for the application and withdraw from the prover balance.
If the order is locked, only the prover that locked the order may receive payment.
If another prover delivers a proof for an order that is locked, this method will revert
unless `paymentRequired` is set to `false` on the `Fulfillment` struct.

```solidity
function fulfillAndWithdraw(Fulfillment calldata fill, AssessorReceipt calldata assessorReceipt)
    public
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
    public
    returns (bytes[] memory paymentError);
```

**Parameters**

| Name              | Type              | Description                                                                                          |
| ----------------- | ----------------- | ---------------------------------------------------------------------------------------------------- |
| `fills`           | `Fulfillment[]`   | The array of fulfillment information.                                                                |
| `assessorReceipt` | `AssessorReceipt` | The Assessor's guest fulfillment information verified to confirm the request's requirements are met. |

### _fulfillAndPay

Complete the fulfillment logic after having verified the app and assessor receipts.

```solidity
function _fulfillAndPay(Fulfillment calldata fill, address prover) internal returns (bytes memory paymentError);
```

### _fulfillAndPayLocked

For a request that is currently locked. Marks the request as fulfilled, and transfers payment if eligible.

_It is possible for anyone to fulfill a request at any time while the request has not expired.
If the request is currently locked, only the prover can fulfill it and receive payment_

```solidity
function _fulfillAndPayLocked(
    RequestLock memory lock,
    RequestId id,
    address client,
    uint32 idx,
    bytes32 requestDigest,
    bool fulfilled,
    address assessorProver
) internal returns (bytes memory paymentError);
```

### _fulfillAndPayWasLocked

For a request that was locked, and now the lock has expired. Marks the request as fulfilled,
and transfers payment if eligible.

_It is possible for anyone to fulfill a request at any time while the request has not expired.
If the request was locked, and now the lock has expired, and the request as a whole has not expired,
anyone can fulfill it and receive payment._

```solidity
function _fulfillAndPayWasLocked(
    RequestLock memory lock,
    RequestId id,
    address client,
    uint32 idx,
    bytes32 requestDigest,
    bool fulfilled,
    address assessorProver
) internal returns (bytes memory paymentError);
```

### _fulfillAndPayNeverLocked

For a request that has never been locked. Marks the request as fulfilled, and transfers payment if eligible.

_If a never locked request is fulfilled, but fails the requirements for payment, no
payment can ever be rendered for this order in the future._

```solidity
function _fulfillAndPayNeverLocked(
    RequestId id,
    address client,
    uint32 idx,
    bytes32 requestDigest,
    bool fulfilled,
    address assessorProver
) internal returns (bytes memory paymentError);
```

### _applyMarketFee

```solidity
function _applyMarketFee(uint96 proverPayment) internal returns (uint96);
```

### _executeCallback

Execute the callback for a fulfilled request if one is specified

_This function is called after payment is processed and handles any callback specified in the request_

```solidity
function _executeCallback(
    RequestId id,
    address callbackAddr,
    uint96 callbackGasLimit,
    bytes32 imageId,
    bytes calldata journal,
    bytes calldata seal
) internal;
```

**Parameters**

| Name               | Type        | Description                                                 |
| ------------------ | ----------- | ----------------------------------------------------------- |
| `id`               | `RequestId` | The ID of the request being fulfilled                       |
| `callbackAddr`     | `address`   | The address of the callback contract                        |
| `callbackGasLimit` | `uint96`    | The gas limit to use for the callback                       |
| `imageId`          | `bytes32`   | The ID of the RISC Zero guest image that produced the proof |
| `journal`          | `bytes`     | The output journal from the RISC Zero guest execution       |
| `seal`             | `bytes`     | The cryptographic seal proving correct execution            |

### submitRoot

Submit a new root to a set-verifier.

_Consider using `submitRootAndFulfillBatch` to submit the root and fulfill in one transaction._

```solidity
function submitRoot(address setVerifierAddress, bytes32 root, bytes calldata seal) external;
```

**Parameters**

| Name                 | Type      | Description                      |
| -------------------- | --------- | -------------------------------- |
| `setVerifierAddress` | `address` |                                  |
| `root`               | `bytes32` | The new merkle root.             |
| `seal`               | `bytes`   | The seal of the new merkle root. |

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

### deposit

Deposit Ether into the market to pay for proof.

_Value deposited is msg.value and it is credited to the account of msg.sender._

```solidity
function deposit() public payable;
```

### _withdraw

```solidity
function _withdraw(address account, uint256 value) internal;
```

### withdraw

Withdraw Ether from the market.

_Value is debited from msg.sender._

```solidity
function withdraw(uint256 value) public;
```

**Parameters**

| Name    | Type      | Description             |
| ------- | --------- | ----------------------- |
| `value` | `uint256` | The amount to withdraw. |

### balanceOf

Check the deposited balance, in Ether, of the given account.

```solidity
function balanceOf(address addr) public view returns (uint256);
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
function withdrawFromTreasury(uint256 value) public onlyOwner;
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

### _depositStake

```solidity
function _depositStake(address from, uint256 value) internal;
```

### withdrawStake

Withdraw stake from the market.

```solidity
function withdrawStake(uint256 value) public;
```

### balanceOfStake

Check the deposited balance, in HP, of the given account.

```solidity
function balanceOfStake(address addr) public view returns (uint256);
```

### withdrawFromStakeTreasury

Withdraw funds from the market' stake treasury.

_Value is debited from the market's account._

```solidity
function withdrawFromStakeTreasury(uint256 value) public onlyOwner;
```

**Parameters**

| Name    | Type      | Description             |
| ------- | --------- | ----------------------- |
| `value` | `uint256` | The amount to withdraw. |

### requestIsFulfilled

Check if the given request has been fulfilled (i.e. a proof was delivered).

```solidity
function requestIsFulfilled(RequestId id) public view returns (bool);
```

**Parameters**

| Name | Type        | Description |
| ---- | ----------- | ----------- |
| `id` | `RequestId` |             |

**Returns**

| Name     | Type   | Description                                        |
| -------- | ------ | -------------------------------------------------- |
| `<none>` | `bool` | True if the request is fulfilled, false otherwise. |

### requestIsLocked

Check if the given request has been locked (i.e. accepted) by a prover.

_When a request is locked, only the prover it is locked to can be paid to fulfill the job._

```solidity
function requestIsLocked(RequestId id) public view returns (bool);
```

**Parameters**

| Name | Type        | Description |
| ---- | ----------- | ----------- |
| `id` | `RequestId` |             |

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
function requestIsSlashed(RequestId id) external view returns (bool);
```

**Parameters**

| Name | Type        | Description |
| ---- | ----------- | ----------- |
| `id` | `RequestId` |             |

**Returns**

| Name     | Type   | Description                                                                |
| -------- | ------ | -------------------------------------------------------------------------- |
| `<none>` | `bool` | True if the request resulted in the prover being slashed, false otherwise. |

### requestLockDeadline

For a given locked request, returns when the lock expires.

_If the request is not locked, this function will revert._

```solidity
function requestLockDeadline(RequestId id) external view returns (uint64);
```

**Parameters**

| Name | Type        | Description |
| ---- | ----------- | ----------- |
| `id` | `RequestId` |             |

**Returns**

| Name     | Type     | Description                                     |
| -------- | -------- | ----------------------------------------------- |
| `<none>` | `uint64` | The expiration time of the lock on the request. |

### requestDeadline

For a given locked request, returns when request expires.

_If the request is not locked, this function will revert._

```solidity
function requestDeadline(RequestId id) external view returns (uint64);
```

**Parameters**

| Name | Type        | Description |
| ---- | ----------- | ----------- |
| `id` | `RequestId` |             |

**Returns**

| Name     | Type     | Description                         |
| -------- | -------- | ----------------------------------- |
| `<none>` | `uint64` | The expiration time of the request. |

### _verifyClientSignature

```solidity
function _verifyClientSignature(ProofRequest calldata request, address addr, bytes calldata clientSignature)
    internal
    view
    returns (bytes32);
```

### _extractProverAddress

```solidity
function _extractProverAddress(bytes32 requestHash, bytes calldata proverSignature) internal pure returns (address);
```

### eip712DomainSeparator

EIP 712 domain separator getter.

```solidity
function eip712DomainSeparator() external view returns (bytes32);
```

**Returns**

| Name     | Type      | Description                   |
| -------- | --------- | ----------------------------- |
| `<none>` | `bytes32` | The EIP 712 domain separator. |

### revertWith

Internal utility function to revert with a pre-encoded error.

```solidity
function revertWith(bytes memory err) internal pure;
```
