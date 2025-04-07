# BoundlessMarketCallback

**Inherits:**
[IBoundlessMarketCallback](/developers/smart-contracts/interface-IBoundlessMarketCallback)

Contract for handling proofs delivered by the Boundless Market's callback mechanism.

_This contract provides a framework for applications to safely handle proofs delivered by
the Boundless Market for a specific image ID. The intention is for developers to inherit the contract
and implement the internal `_handleProof` function._

_We recommend a best practice of "trust but verify" whenever receiving proofs, so we verify the proof
here even though the Boundless Market already verifies the proof as part of its fulfillment process.
Proof verification in Boundless is cheap as it is just a merkle proof, so this adds minimal overhead._

## State Variables

### VERIFIER

```solidity
IRiscZeroVerifier public immutable VERIFIER;
```

### BOUNDLESS_MARKET

```solidity
address public immutable BOUNDLESS_MARKET;
```

### IMAGE_ID

```solidity
bytes32 public immutable IMAGE_ID;
```

## Functions

### constructor

Initializes the callback contract with verifier and market addresses

```solidity
constructor(IRiscZeroVerifier verifier, address boundlessMarket, bytes32 imageId);
```

**Parameters**

| Name              | Type                | Description                             |
| ----------------- | ------------------- | --------------------------------------- |
| `verifier`        | `IRiscZeroVerifier` | The RISC Zero verifier contract address |
| `boundlessMarket` | `address`           | The BoundlessMarket contract address    |
| `imageId`         | `bytes32`           | The image ID to accept proofs of.       |

### handleProof

Handles submitting proofs with RISC Zero proof verification

_If not called by BoundlessMarket, the function MUST verify the proof before proceeding._

```solidity
function handleProof(bytes32 imageId, bytes calldata journal, bytes calldata seal) public;
```

**Parameters**

| Name      | Type      | Description                                                 |
| --------- | --------- | ----------------------------------------------------------- |
| `imageId` | `bytes32` | The ID of the RISC Zero guest image that produced the proof |
| `journal` | `bytes`   | The output journal from the RISC Zero guest execution       |
| `seal`    | `bytes`   | The cryptographic seal proving correct execution            |

### _handleProof

Internal function to be implemented by inheriting contracts

_Override this function to implement custom proof handling logic_

```solidity
function _handleProof(bytes32 imageId, bytes calldata journal, bytes calldata seal) internal virtual;
```

**Parameters**

| Name      | Type      | Description                                                 |
| --------- | --------- | ----------------------------------------------------------- |
| `imageId` | `bytes32` | The ID of the RISC Zero guest image that produced the proof |
| `journal` | `bytes`   | The output journal from the RISC Zero guest execution       |
| `seal`    | `bytes`   | The cryptographic seal proving correct execution            |
