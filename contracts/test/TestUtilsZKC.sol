// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

pragma solidity ^0.8.26;

import {ZKC} from "zkc/ZKC.sol";
import {veZKC} from "zkc/veZKC.sol";
import {StakingRewards} from "zkc/rewards/StakingRewards.sol";

// Contracts definitions to force forge build to build the json for these contracts
// so that they can be used by the test-utils crate

contract TestUtilsZKC is ZKC {}

contract TestUtilsVeZKC is veZKC {}

contract TestUtilsStakingRewards is StakingRewards {}
