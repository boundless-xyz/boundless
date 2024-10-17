# ProofMarket Contract Deployment Using UUPS Pattern

## Overview

The Boundless market is deployed and upgraded using the **UUPS (Universal Upgradeable Proxy Standard)** proxy pattern. The UUPS pattern is a mechanism that allows contracts to be upgraded over time while maintaining their state, thus enabling flexible and modular smart contract development.

### Functions:
- **`initialize()`**: This function initializes the contract with ownership and other parameters like the verifier, assessor ID, and assessor guest URL. It ensures that these values are set properly during the initial deployment.
- **`upgrade()`**: This function may be used to update the assessor information after upgrading the implementation contract.
- **`_authorizeUpgrade()`**: This function secures the upgrade process by ensuring only the contract owner can authorize an upgrade.

## Usage Instructions

The deployment and upgrade process is managed using a Forge script written in Solidity. Below is an overview of how the `Deploy` script operates:

### Script Overview (`Deploy` Contract)

- **Configuration Management**: The script reads from a TOML configuration file [`config.toml`](./config.toml) to get deployment settings such as contract addresses and other parameters. 
- **Admin Key**: The script loads a private key from the environment variable `PRIVATE_KEY` to sign transactions for deployment and upgrades. This should be assumed to be the owner.
- **Verifier and SetVerifier Deployment**:
  - If a verifier or set verifier contract is already deployed (as specified in the config), the script will use those addresses. Otherwise, it will deploy new contracts.
- **Proof Market (Proxy) Deployment and Upgrade**:
  - The script deploys a new implementation of the `ProofMarket` contract and wraps it in a UUPS proxy if it is not already deployed.
  - If a proxy already exists, the script upgrades the existing proxy to use the new implementation contract. Note that, in case of upgrade,
  the `VERSION` of the `ProofMarket` contract should first get bumped. That's because the `reinitializer` modifier can only be invoked once per version.
- **Local Testing Mode**: The script has logic to handle local development (e.g., setting file-based URLs for local environments).

### How to run:

Make sure you update your [.env](../../.env) file by setting `RPC_URL` and `PRIVATE_KEY`, and your [config.toml](./config.toml) accordingly to your environment/profile.

Then run:

```bash
forge script contracts/scripts/Deploy.s.sol --rpc-url $RPC_URL --broadcast -vv
```





