# HitPoints

**Inherits:**
ERC20, ERC20Burnable, ERC20Permit, [IHitPoints](/developers/smart-contracts/interface-IHitPoints), AccessControl, Ownable

Implementation of a restricted transfer token using ERC20

## State Variables

### MAX_BALANCE

```solidity
uint256 constant MAX_BALANCE = type(uint96).max;
```

### MINTER

```solidity
bytes32 public constant MINTER = keccak256("MINTER");
```

### AUTHORIZED_TRANSFER

```solidity
bytes32 public constant AUTHORIZED_TRANSFER = keccak256("AUTHORIZED_TRANSFER");
```

## Functions

### constructor

```solidity
constructor(address initialOwner) ERC20("HitPoints", "HP") ERC20Permit("HitPoints") Ownable(initialOwner);
```

### transferOwnership

_Transfers ownership of the contract to a new account (`newOwner`).
Can only be called by the current owner._

```solidity
function transferOwnership(address newOwner) public override onlyOwner;
```

### grantMinterRole

Grants the MINTER role to an account

_This role is used to allow minting new tokens_

```solidity
function grantMinterRole(address account) external onlyOwner;
```

**Parameters**

| Name      | Type      | Description                                   |
| --------- | --------- | --------------------------------------------- |
| `account` | `address` | The address that will receive the minter role |

### revokeMinterRole

Revokes the MINTER role from an account

```solidity
function revokeMinterRole(address account) external onlyOwner;
```

**Parameters**

| Name      | Type      | Description                                |
| --------- | --------- | ------------------------------------------ |
| `account` | `address` | The address that will lose the minter role |

### grantAuthorizedTransferRole

Grants the AUTHORIZED_TRANSFER role to an account

_This role is used to allow transfers from/to an address_

```solidity
function grantAuthorizedTransferRole(address account) external onlyOwner;
```

**Parameters**

| Name      | Type      | Description                                                |
| --------- | --------- | ---------------------------------------------------------- |
| `account` | `address` | The address that will receive the authorized transfer role |

### revokeAuthorizedTransferRole

Revokes the AUTHORIZED_TRANSFER role from an account

```solidity
function revokeAuthorizedTransferRole(address account) external onlyOwner;
```

**Parameters**

| Name      | Type      | Description                                             |
| --------- | --------- | ------------------------------------------------------- |
| `account` | `address` | The address that will lose the authorized transfer role |

### mint

Creates new tokens and assigns them to an account

```solidity
function mint(address account, uint256 value) external onlyRole(MINTER);
```

**Parameters**

| Name      | Type      | Description                                     |
| --------- | --------- | ----------------------------------------------- |
| `account` | `address` | The address that will receive the minted tokens |
| `value`   | `uint256` | The `value` amount of tokens to mint            |

### _update

```solidity
function _update(address from, address to, uint256 value) internal virtual override;
```
