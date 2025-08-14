// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.
pragma solidity ^0.8.24;

using CallbackDataLibrary for CallbackData global;

/// @title Callback Struct and Library
/// @notice Represents a callback configuration for proof delivery
struct CallbackData {
    /// @notice Image ID of the guest that was verifiably executed to satisfy the request.
    bytes32 imageId;
    /// @notice Journal committed by the guest program execution.
    /// @dev The journal is checked to satisfy the predicate specified on the request's requirements.
    bytes journal;
}

library CallbackDataLibrary {}
