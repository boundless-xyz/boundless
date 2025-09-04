// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

//! Smart contract interfaces and bytecode for ZKC contracts.

alloy::sol!(
    #![sol(rpc, all_derives)]
    "src/contracts/artifacts/IZKC.sol"
);

alloy::sol!(
    #![sol(rpc, all_derives)]
    "src/contracts/artifacts/IStaking.sol"
);

alloy::sol!(
    #![sol(rpc, all_derives)]
    "src/contracts/artifacts/IRewards.sol"
);
