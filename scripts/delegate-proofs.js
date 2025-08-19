#!/usr/bin/env node

const { ethers } = require('ethers');
require('dotenv').config();

// This script is part of the Boundless repository
// Usage: node scripts/delegate-proofs.js <command> [args]

// ProofDelegation contract ABI (minimal)
const PROOF_DELEGATION_ABI = [
    "function delegateProofs(address prover, address target, uint256 deadline, bytes calldata signature) external",
    "function getDelegatedProver(address target) external view returns (address)",
    "function hasDelegatedProofs(address target) external view returns (bool)",
    "function nonces(address prover) external view returns (uint256)",
    "event ProofDelegated(address indexed prover, address indexed target, uint256 nonce)"
];

// EIP-712 domain for ProofDelegation contract
const DOMAIN = {
    name: 'ProofDelegation',
    version: '1',
    chainId: 8453, // Base mainnet
    verifyingContract: process.env.PROOF_DELEGATION_ADDRESS
};

// EIP-712 types
const TYPES = {
    Delegation: [
        { name: 'prover', type: 'address' },
        { name: 'target', type: 'address' },
        { name: 'nonce', type: 'uint256' },
        { name: 'deadline', type: 'uint256' }
    ]
};

async function main() {
    const args = process.argv.slice(2);
    const command = args[0];
    
    if (!process.env.PRIVATE_KEY) {
        console.error('❌ PRIVATE_KEY environment variable is required');
        console.log('Please add PRIVATE_KEY to your .env file (same as your Boundless service)');
        process.exit(1);
    }
    
    if (!process.env.RPC_URL) {
        console.error('❌ RPC_URL environment variable is required');
        console.log('Please add RPC_URL to your .env file (same as your Boundless service)');
        process.exit(1);
    }
    
    if (!process.env.PROOF_DELEGATION_ADDRESS) {
        console.error('❌ PROOF_DELEGATION_ADDRESS environment variable is required');
        console.log('Usage: PROOF_DELEGATION_ADDRESS=0x... node delegate-proofs.js <command> [args]');
        process.exit(1);
    }
    
    const provider = new ethers.JsonRpcProvider(process.env.RPC_URL);
    const wallet = new ethers.Wallet(process.env.PRIVATE_KEY, provider);
    const contract = new ethers.Contract(process.env.PROOF_DELEGATION_ADDRESS, PROOF_DELEGATION_ABI, wallet);
    
    switch (command) {
        case 'delegate':
            if (args.length < 2) {
                console.error('❌ Target address required');
                console.log('Usage: node delegate-proofs.js delegate <target-address> [deadline]');
                process.exit(1);
            }
            await delegateProofs(contract, wallet, args[1], args[2]);
            break;
            
        case 'check':
            if (args.length < 2) {
                console.error('❌ Address required');
                console.log('Usage: node delegate-proofs.js check <address>');
                process.exit(1);
            }
            await checkDelegation(contract, args[1]);
            break;
            
        default:
            console.log('Boundless Proof Delegation Tool');
            console.log('');
            console.log('Commands:');
            console.log('  delegate <target-address> [deadline]  - Delegate all proof solving to target address');
            console.log('  check <address>                       - Check delegation status');
            console.log('');
            console.log('Environment variables (from your .env file):');
            console.log('  PRIVATE_KEY                          - Your prover wallet private key');
            console.log('  RPC_URL                              - Ethereum RPC URL');
            console.log('  PROOF_DELEGATION_ADDRESS             - ProofDelegation contract address');
            console.log('');
            console.log('Examples:');
            console.log('  node delegate-proofs.js delegate 0x1234...your_main_wallet');
            console.log('  node delegate-proofs.js check 0x1234...your_main_wallet');
            console.log('');
            console.log('Note: Delegations are permanent and cannot be revoked.');
    }
}

async function delegateProofs(contract, wallet, targetAddress, deadline) {
    try {
        console.log('🔍 Delegating all proof solving...');
        console.log(`   Prover: ${wallet.address}`);
        console.log(`   Target: ${targetAddress}`);
        console.log('   ⚠️  This delegation is PERMANENT and cannot be revoked');
        
        // Get current nonce
        const nonce = await contract.nonces(wallet.address);
        console.log(`   Nonce: ${nonce}`);
        
        // Set deadline (default to 1 hour from now)
        const deadlineTime = deadline ? parseInt(deadline) : Math.floor(Date.now() / 1000) + 3600;
        console.log(`   Deadline: ${new Date(deadlineTime * 1000).toISOString()}`);
        
        // Create EIP-712 signature
        const signatureData = {
            prover: wallet.address,
            target: targetAddress,
            nonce: nonce,
            deadline: deadlineTime
        };
        
        const signature = await wallet.signTypedData(DOMAIN, TYPES, signatureData);
        console.log(`   Signature: ${signature}`);
        
        // Submit delegation
        console.log('📝 Submitting permanent delegation transaction...');
        const tx = await contract.delegateProofs(
            wallet.address,
            targetAddress,
            deadlineTime,
            signature
        );
        
        console.log(`   Transaction hash: ${tx.hash}`);
        console.log('⏳ Waiting for confirmation...');
        
        const receipt = await tx.wait();
        console.log('✅ Permanent delegation successful!');
        console.log(`   Block: ${receipt.blockNumber}`);
        console.log('   All your proof solving will now be accredited to the target address');
        
    } catch (error) {
        console.error('❌ Delegation failed:', error.message);
        process.exit(1);
    }
}



async function checkDelegation(contract, address) {
    try {
        console.log('🔍 Checking delegation status...');
        console.log(`   Address: ${address}`);
        
        const hasDelegated = await contract.hasDelegatedProofs(address);
        const delegatedProver = await contract.getDelegatedProver(address);
        
        if (hasDelegated) {
            console.log('✅ Address has delegated proof solving');
            console.log(`   Accredited from: ${delegatedProver}`);
            console.log('   This delegation is permanent');
        } else {
            console.log('❌ Address has no delegated proof solving');
        }
        
    } catch (error) {
        console.error('❌ Check failed:', error.message);
        process.exit(1);
    }
}

main().catch(console.error); 