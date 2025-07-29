# Proof Delegation Contribution

This contribution adds proof delegation functionality to Boundless, allowing provers to securely delegate their proof solving achievements to another address for campaign participation.

## Problem Solved

Users running Boundless provers face a security dilemma:
- **Security**: They use dedicated wallets for proving (to avoid exposing private keys on servers)
- **Campaign**: The Boundless Signal campaign requires using their main wallet to claim points
- **Mismatch**: The proofs are on the prover wallet, but points need to be claimed on the main wallet

## Solution

A **Proof Delegation Contract** that allows provers to securely delegate their proof solving to another address using EIP-712 signatures.

### Key Features

- ✅ **Secure**: Uses EIP-712 signatures - no private key exposure required
- ✅ **Simple**: Uses existing .env file from Boundless service
- ✅ **Permanent**: Delegations are permanent for ease of use
- ✅ **Comprehensive**: Delegates all proof solving, not specific proofs
- ✅ **Fast**: Can be deployed and used immediately
- ✅ **Compatible**: Works with existing Boundless infrastructure

## Files Added/Modified

### New Files
- `contracts/src/ProofDelegation.sol` - The delegation contract
- `scripts/delegate-proofs.js` - User-friendly delegation script
- `scripts/deploy.js` - Deployment script

### Modified Files
- `crates/boundless-cli/src/bin/boundless.rs` - Added delegation CLI commands
- `crates/boundless-market/src/deployments.rs` - Added delegation contract address field

## Usage

### Deploy Contract
```bash
forge create ProofDelegation --rpc-url https://mainnet.base.org --private-key YOUR_PRIVATE_KEY
```

### Delegate Proof Solving
```bash
# Using the script
node scripts/delegate-proofs.js delegate 0x1234...your_main_wallet

# Using the CLI (after deployment)
boundless account delegate-proofs --target-address 0x1234...your_main_wallet
```

### Check Delegation
```bash
node scripts/delegate-proofs.js check 0x1234...your_main_wallet
```

## Environment Variables

Add to your existing `.env` file (same as your Boundless service):
```env
PRIVATE_KEY=0x...your_prover_wallet_private_key
RPC_URL=https://mainnet.base.org
PROOF_DELEGATION_ADDRESS=0x...deployed_contract_address
```

## Contract Details

### ProofDelegation.sol
```solidity
contract ProofDelegation is EIP712, Ownable {
    // Delegate all proof solving to target (permanent)
    function delegateProofs(
        address prover,
        address target,
        uint256 deadline,
        bytes calldata signature
    ) external;
    
    // Check if address has delegated proof solving
    function hasDelegatedProofs(address target) external view returns (bool);
}
```

### Security Features
- **EIP-712 Signatures**: Secure, standardized signature format
- **Nonce Protection**: Prevents replay attacks
- **Deadline Support**: Time-limited signatures
- **Permanent Delegations**: Once delegated, cannot be revoked for simplicity
- **One-Time Only**: Each prover can only delegate once

## Integration with Campaign System

The campaign system needs to be updated to check for delegated proof solving:

```javascript
// Check for delegated proof solving
const delegationContract = new ethers.Contract(DELEGATION_ADDRESS, ABI, provider);
const hasDelegated = await delegationContract.hasDelegatedProofs(userAddress);

if (hasDelegated) {
    const proverAddress = await delegationContract.getDelegatedProver(userAddress);
    const delegatedProofs = await getProofDeliveredEvents(proverAddress, provider);
    // Include delegated proofs in point calculation
}
```

## Testing

### Local Testing
```bash
# Start local node
anvil

# Deploy to local network
forge create ProofDelegation --rpc-url http://localhost:8545 --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80

# Test delegation
PRIVATE_KEY=0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
RPC_URL=http://localhost:8545 \
PROOF_DELEGATION_ADDRESS=0x... \
node scripts/delegate-proofs.js delegate 0x70997970C51812dc3A010C7d01b50e0d17dc79C8
```

## Backward Compatibility

All changes are **backward compatible**:
- ✅ Optional delegation contract address in deployments
- ✅ Existing CLI commands continue to work
- ✅ No breaking changes to existing functionality
- ✅ Existing deployments work without modification

## Security Considerations

1. **Private Key Security**: Script requires prover's private key, but only for signing delegation
2. **Signature Verification**: Contract verifies EIP-712 signatures to ensure only the prover can delegate
3. **Nonce Protection**: Each delegation uses a unique nonce to prevent replay attacks
4. **Permanent Delegations**: Once delegated, cannot be revoked for simplicity

## License

This contribution follows the same license as the Boundless project. 