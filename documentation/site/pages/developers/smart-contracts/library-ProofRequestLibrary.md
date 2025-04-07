# ProofRequestLibrary

## State Variables

### PROOF_REQUEST_TYPE

_Id is uint256 as for user defined types, the eip712 type hash uses the underlying type._

```solidity
string constant PROOF_REQUEST_TYPE =
    "ProofRequest(uint256 id,Requirements requirements,string imageUrl,Input input,Offer offer)";
```

### PROOF_REQUEST_TYPEHASH

```solidity
bytes32 constant PROOF_REQUEST_TYPEHASH = keccak256(
    abi.encodePacked(
        PROOF_REQUEST_TYPE,
        CallbackLibrary.CALLBACK_TYPE,
        InputLibrary.INPUT_TYPE,
        OfferLibrary.OFFER_TYPE,
        PredicateLibrary.PREDICATE_TYPE,
        RequirementsLibrary.REQUIREMENTS_TYPE
    )
);
```

## Functions

### eip712Digest

Computes the EIP-712 digest for the given proof request.

```solidity
function eip712Digest(ProofRequest memory request) internal pure returns (bytes32);
```

**Parameters**

| Name      | Type           | Description                                  |
| --------- | -------------- | -------------------------------------------- |
| `request` | `ProofRequest` | The proof request to compute the digest for. |

**Returns**

| Name     | Type      | Description                              |
| -------- | --------- | ---------------------------------------- |
| `<none>` | `bytes32` | The EIP-712 digest of the proof request. |

### validate

Validates the proof request with the intention for it to be priced.
Does not check if the request is already locked or fulfilled, but does check
if it has expired.

```solidity
function validate(ProofRequest calldata request) internal view returns (uint64 lockDeadline, uint64 deadline);
```

**Parameters**

| Name      | Type           | Description                    |
| --------- | -------------- | ------------------------------ |
| `request` | `ProofRequest` | The proof request to validate. |

**Returns**

| Name           | Type     | Description                                           |
| -------------- | -------- | ----------------------------------------------------- |
| `lockDeadline` | `uint64` | The deadline for when a lock expires for the request. |
| `deadline`     | `uint64` | The deadline for the request as a whole.              |
