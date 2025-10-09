// SPDX-License-Identifier: MIT
pragma solidity 0.8.26;

import {IVotes as OZIVotes} from "@openzeppelin/contracts/interfaces/IERC5805.sol";

/// @title IVotes
/// @notice Interface that extends OpenZeppelin's IVotes interface with custom errors
/// @dev This allows us to extend the standard IVotes interface in the future if needed
interface IVotes is OZIVotes {
    // Custom errors
    error CannotDelegateVotesWhileWithdrawing();

    // This interface extends OpenZeppelin's IVotes interface
}
