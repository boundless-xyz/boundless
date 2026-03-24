// Copyright 2026 Boundless Foundation, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

pragma solidity ^0.8.26;

/// @title IVersionRegistry
/// @notice Interface for the on-chain registry storing the minimum acceptable broker version.
///         The broker reads this contract on startup and periodically.
///         If the broker version is below the minimum, the broker shuts down.
interface IVersionRegistry {
    /// @notice Emitted when the minimum broker version is updated.
    event MinimumBrokerVersionUpdated(uint64 oldVersion, uint64 newVersion);

    /// @notice Emitted when the governance notice is updated.
    event NoticeUpdated(string oldNotice, string newNotice);

    /// @notice Thrown when the provided version value exceeds the valid 48-bit semver range.
    error InvalidVersion();

    /// @notice Returns the minimum acceptable broker version, encoded as (major << 32) | (minor << 16) | patch.
    function minimumBrokerVersion() external view returns (uint64);

    /// @notice Returns the current governance notice string (may be empty).
    function notice() external view returns (string memory);

    /// @notice Set a governance notice message displayed to brokers on each version check.
    ///         Set to empty string to clear. Independent of version — can be set/cleared any time.
    function setNotice(string calldata _notice) external;

    /// @notice Returns both the minimum version and notice in a single call.
    function getVersionInfo() external view returns (uint64 minimumVersion, string memory _notice);

    /// @notice Set the minimum broker version using a packed uint64.
    /// @param _version Packed version: (major << 32) | (minor << 16) | patch. Must not exceed 48 bits.
    function setMinimumBrokerVersion(uint64 _version) external;

    /// @notice Set the minimum broker version using individual semver components.
    /// @param major Major version component.
    /// @param minor Minor version component.
    /// @param patch Patch version component.
    function setMinimumBrokerVersionSemver(uint16 major, uint16 minor, uint16 patch) external;

    /// @notice Returns the minimum version as (major, minor, patch).
    function minimumBrokerVersionSemver() external view returns (uint16 major, uint16 minor, uint16 patch);

    /// @notice Pack semver components into a single uint64.
    ///         Preserves natural ordering: packVersion(a,b,c) > packVersion(d,e,f) iff (a,b,c) > (d,e,f).
    function packVersion(uint16 major, uint16 minor, uint16 patch) external pure returns (uint64);

    /// @notice Unpack a uint64 version into semver components.
    function unpackVersion(uint64 _version) external pure returns (uint16 major, uint16 minor, uint16 patch);

    /// @notice Returns the minimum version as a human-readable string (e.g. "2.3.1").
    function minimumBrokerVersionString() external view returns (string memory);
}
