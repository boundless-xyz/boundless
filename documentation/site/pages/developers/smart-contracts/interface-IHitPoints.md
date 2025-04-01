# IHitPoints

Interface of a restricted transfer token using ERC20


## Functions
### grantMinterRole

Grants the MINTER role to an account

*This role is used to allow minting new tokens*


```solidity
function grantMinterRole(address account) external;
```
**Parameters**

|Name|Type|Description|
|----|----|-----------|
|`account`|`address`|The address that will receive the minter role|


### revokeMinterRole

Revokes the MINTER role from an account


```solidity
function revokeMinterRole(address account) external;
```
**Parameters**

|Name|Type|Description|
|----|----|-----------|
|`account`|`address`|The address that will lose the minter role|


### grantAuthorizedTransferRole

Grants the AUTHORIZED_TRANSFER role to an account

*This role is used to allow transfers from/to an address*


```solidity
function grantAuthorizedTransferRole(address account) external;
```
**Parameters**

|Name|Type|Description|
|----|----|-----------|
|`account`|`address`|The address that will receive the authorized transfer role|


### revokeAuthorizedTransferRole

Revokes the AUTHORIZED_TRANSFER role from an account


```solidity
function revokeAuthorizedTransferRole(address account) external;
```
**Parameters**

|Name|Type|Description|
|----|----|-----------|
|`account`|`address`|The address that will lose the authorized transfer role|


### mint

Creates new tokens and assigns them to an account


```solidity
function mint(address account, uint256 value) external;
```
**Parameters**

|Name|Type|Description|
|----|----|-----------|
|`account`|`address`|The address that will receive the minted tokens|
|`value`|`uint256`|The `value` amount of tokens to mint|


## Errors
### UnauthorizedTransfer
*Thrown when trying to transfer tokens from/to an unauthorized address*


```solidity
error UnauthorizedTransfer();
```

### BalanceExceedsLimit
*Thrown when balance exceeds uint96 max*


```solidity
error BalanceExceedsLimit(address account, uint256 currentBalance, uint256 addedAmount);
```

