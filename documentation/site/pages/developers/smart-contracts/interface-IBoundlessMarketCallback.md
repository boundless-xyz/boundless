# IBoundlessMarketCallback

Interface for handling proof callbacks from BoundlessMarket with proof verification

*Inherit from this contract to implement custom proof handling logic for BoundlessMarket proofs*


## Functions
### handleProof

Handles submitting proofs with RISC Zero proof verification

*If not called by BoundlessMarket, the function MUST verify the proof before proceeding.*


```solidity
function handleProof(bytes32 imageId, bytes calldata journal, bytes calldata seal) external;
```
**Parameters**

|Name|Type|Description|
|----|----|-----------|
|`imageId`|`bytes32`|The ID of the RISC Zero guest image that produced the proof|
|`journal`|`bytes`|The output journal from the RISC Zero guest execution|
|`seal`|`bytes`|The cryptographic seal proving correct execution|


