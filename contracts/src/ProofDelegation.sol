// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/utils/cryptography/ECDSA.sol";
import "@openzeppelin/contracts/utils/cryptography/EIP712.sol";
import "@openzeppelin/contracts/access/Ownable.sol";

/**
 * @title ProofDelegation
 * @notice Allows provers to delegate their proof achievements to another address
 * @dev Uses EIP-712 signatures for secure delegation without requiring private key exposure
 */
contract ProofDelegation is EIP712, Ownable {
    using ECDSA for bytes32;

    // EIP-712 domain separator
    bytes32 public constant DELEGATION_TYPEHASH = keccak256(
        "Delegation(address prover,address target,uint256 nonce,uint256 deadline)"
    );

    // Mapping from target address to prover address
    mapping(address => address) public delegatedProvers;
    
    // Mapping from prover address to target address
    mapping(address => address) public proverTargets;
    
    // Nonce tracking for replay protection
    mapping(address => uint256) public nonces;

    // Events
    event ProofDelegated(address indexed prover, address indexed target, uint256 nonce);

    constructor() EIP712("ProofDelegation", "1") Ownable(msg.sender) {}

    /**
     * @notice Delegate all proof solving to target address (permanent)
     * @param prover The address that solves proofs
     * @param target The address to accredit proof solving to
     * @param deadline The deadline for the signature
     * @param signature The EIP-712 signature from the prover
     */
    function delegateProofs(
        address prover,
        address target,
        uint256 deadline,
        bytes calldata signature
    ) external {
        require(block.timestamp <= deadline, "ProofDelegation: signature expired");
        require(target != address(0), "ProofDelegation: invalid target address");
        require(prover != address(0), "ProofDelegation: invalid prover address");
        require(prover != target, "ProofDelegation: cannot delegate to self");
        require(proverTargets[prover] == address(0), "ProofDelegation: already delegated");

        uint256 nonce = nonces[prover]++;
        
        bytes32 structHash = keccak256(
            abi.encode(DELEGATION_TYPEHASH, prover, target, nonce, deadline)
        );
        
        bytes32 hash = _hashTypedDataV4(structHash);
        address signer = hash.recover(signature);
        
        require(signer == prover, "ProofDelegation: invalid signature");

        delegatedProvers[target] = prover;
        proverTargets[prover] = target;

        emit ProofDelegated(prover, target, nonce);
    }

    /**
     * @notice Revoke a delegation (only by the prover or owner)
     * @param prover The prover address
     * @dev DEPRECATED: Delegations are now permanent for simplicity
     */
    function revokeDelegation(address prover) external {
        revert("ProofDelegation: delegations are permanent");
    }

    /**
     * @notice Check if an address has delegated proofs
     * @param target The address to check
     * @return The prover address if delegated, address(0) otherwise
     */
    function getDelegatedProver(address target) external view returns (address) {
        return delegatedProvers[target];
    }

    /**
     * @notice Check if a prover has delegated their proofs
     * @param prover The prover address to check
     * @return The target address if delegated, address(0) otherwise
     */
    function getProverTarget(address prover) external view returns (address) {
        return proverTargets[prover];
    }

    /**
     * @notice Check if an address has proof delegation rights
     * @param target The address to check
     * @return True if the address has delegated proofs
     */
    function hasDelegatedProofs(address target) external view returns (bool) {
        return delegatedProvers[target] != address(0);
    }
} 