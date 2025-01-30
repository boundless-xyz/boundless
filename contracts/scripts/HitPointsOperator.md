# HitPoints Operator Guide

This guide explains how to use the provided Bash script to interact with the **HitPoints** smart contract. It covers prerequisites, environment variables, and usage for the various commands.

---

## 1. Overview

The Bash script offers a way to manage the **HitPoints** ERC20 token by calling specific functions on the deployed smart contract via **cast** (from the Foundry suite). Its main functions include:

- Minting tokens
- Granting and revoking roles (MINTER and AUTHORIZED_TRANSFER)
- Checking token balances

---

## 2. Prerequisites

1. **Foundry / cast**  
   Make sure you have [Foundry](https://book.getfoundry.sh/) installed, which includes the `cast` tool for interacting with Ethereum contracts.

---

## 3. Environment Variables

Before you run the script, you must set the following environment variables in your shell session. If these variables are not set, the script will not run and will display an error.

| Variable             | Description                                                                                                   |
|----------------------|---------------------------------------------------------------------------------------------------------------|
| `PRIVATE_KEY`        | The private key of the account that will send transactions (contract owner/admin or authorized).            |
| `RPC_URL`            | The RPC endpoint of the Ethereum network you’re interacting with (e.g., Infura or Alchemy endpoint).         |
| `HIT_POINTS_ADDRESS` | The contract address where the **HitPoints** token has been deployed.                                         |

Example of setting them in a Unix shell:
```bash
export PRIVATE_KEY=0x1234567890...
export RPC_URL=https://rpc.sepolia.org
export HIT_POINTS_ADDRESS=0xe5321cF13B07Bf6f6dD621E85E45C8e28adedCc9
```

## 4. Usage

### 4.1 Available Commands

#### mint

Calls the mint(address, uint256) function to mint HP tokens to target_address.

```bash
./hp mint <target_address> [amount]
```

Parameters:

* target_address: The address to receive the newly minted tokens.
* amount (optional): The amount of tokens to mint. If omitted, the script defaults to DEFAULT_MINT_AMOUNT (100 tokens by default; 1 token = 1e18 for an 18-decimal token).

Example:

```console
./hp mint 0xRecipientAddress 100000000000000000000
```

This will mint 100 tokens (1 token = 1e18 for an 18-decimal token).


#### grant-minter-role

Calls grantMinterRole(address) on the contract to give the MINTER role to target_address. Addresses with this role can mint tokens.

```bash
./hp grant-minter-role <target_address>
```

Example:

```console
./hp grant-minter-role 0xMinterAddress
```

#### revoke-minter-role

Calls revokeMinterRole(address) on the contract to remove the MINTER role from target_address.

```bash
./hp revoke-minter-role <target_address>
```

Example:

```console
./hp revoke-minter-role 0xMinterAddress
```

#### grant-auth-transfer-role

Calls grantAuthorizedTransferRole(address) to give the AUTHORIZED_TRANSFER role to target_address. Addresses with this role can bypass restricted transfer rules.

```bash
./hp grant-auth-transfer-role <target_address>
```

Example:

```console
./hp grant-auth-transfer-role 0xAuthTransferAddress
```

#### revoke-auth-transfer-role

Calls revokeAuthorizedTransferRole(address) to remove the AUTHORIZED_TRANSFER role from target_address.

```bash
./hp revoke-auth-transfer-role <target_address>
```

Example:

```console
./hp revoke-auth-transfer-role 0xAuthTransferAddress
```

#### Check balance

Calls balanceOf(address) to retrieve the HP token balance of target_address.

```bash
./hp balance <target_address>
```

Example:

```console
./hp balance 0xRecipientAddress
```
