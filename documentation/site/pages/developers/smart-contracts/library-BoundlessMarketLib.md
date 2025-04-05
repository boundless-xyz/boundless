# BoundlessMarketLib

## State Variables

### EIP712_DOMAIN

```solidity
string constant EIP712_DOMAIN = "IBoundlessMarket";
```

### EIP712_DOMAIN_VERSION

```solidity
string constant EIP712_DOMAIN_VERSION = "1";
```

## Functions

### encodeConstructorArgs

ABI encode the constructor args for this contract.

_This function exists to provide a type-safe way to ABI-encode constructor args, for
use in the deployment process with OpenZeppelin Upgrades. Must be kept in sync with the
signature of the BoundlessMarket constructor._

```solidity
function encodeConstructorArgs(IRiscZeroVerifier verifier, bytes32 assessorId) internal pure returns (bytes memory);
```
