# Boundless Market Version Management

## Overview

The Boundless [BoundlessMarket](./src/BoundlessMarket.sol) contract is deployed and upgraded using an [ERC-1967][erc-1967] storage proxy and the [UUPS][uups] (Universal Upgradeable Proxy Standard) proxy pattern.
This pattern allows to upgrade the contract logic while keeping all the state, providing flexibility to improve the implementation over time without service interruptions or manual migration processes.

The Boundless market uses the RISC Zero verifier router, building upon the [verifier versioning system][verifier-versioning].

```mermaid
---
title: Smart Contract Relationships
---
flowchart LR
  subgraph IRiscZeroVerifier
    subgraph routers["Routers"]
        router["RiscZeroVerifierRouter [managed]"]
    end

    subgraph emergencyStops["Emergency Stop Proxies"]
      groth16v1ES["RiscZeroVerifierEmergencyStop"]
      groth16v2ES["RiscZeroVerifierEmergencyStop"]
      setv1ES["RiscZeroVerifierEmergencyStop"]
      setv2ES["RiscZeroVerifierEmergencyStop"]
    end

    subgraph impls["Base Implementations"]
        groth16v1["RiscZeroGroth16Verifier [v1]"]
        groth16v2["RiscZeroGroth16Verifier [v2]"]
        setv1["RiscZeroSetVerifier [v1]"]
        setv2["RiscZeroSetVerifier [v2]"]
    end

    router -- calls --> groth16v1ES
    router -- calls --> groth16v2ES
    router -- calls --> setv1ES
    router -- calls --> setv2ES

    groth16v1ES -- calls --> groth16v1
    groth16v2ES -- calls --> groth16v2
    setv1ES -- calls --> setv1
    setv2ES -- calls --> setv2
  end
  subgraph Boundless market
    subgraph proxy["ERC1967Proxy"]
        marketProxy["BoundlessMarket [proxy]"]
    end
    subgraph BoundlessMarket["Boundless market implementations"]
        boundlessMarketv1["BoundlessMarket [v1]"]
        boundlessMarketv2["BoundlessMarket [v2]"]
    end

    marketProxy -- calls --> boundlessMarketv2
  end
  timelock[TimelockController]
  multisig["RISC Zero Multisig"]

  timelock -- admin --> router
  multisig -- proposer --> timelock
  multisig -- guardian --> emergencyStops
  multisig -- upgrade --> proxy
  setv1 -- calls --> router
  setv2 -- calls --> router
  boundlessMarketv2 -- calls --> router
```

[erc-1967]: https://github.com/OpenZeppelin/openzeppelin-contracts/blob/master/contracts/proxy/ERC1967/ERC1967Proxy.sol
[uups]: https://docs.openzeppelin.com/contracts/5.x/api/proxy#UUPSUpgradeable
[verifier-versioning]: https://github.com/risc0/risc0-ethereum/blob/main/contracts/version-management-design.md
