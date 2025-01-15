// SPDX-License-Identifier: MIT

pragma solidity ^0.8.20;

library SigUtils {
    // keccak256("Permit(address owner,address spender,uint256 value,uint256 nonce,uint256 deadline)");
    bytes32 public constant PERMIT_TYPEHASH = 0x6e71edae12b1b97f4d1f60370fef10105fa2faae0126114a169c64845d6126c9;

    // computes the hash of a permit
    function getPermitHash(address owner, address spender, uint256 value, uint256 nonce, uint256 deadline)
        public
        pure
        returns (bytes32)
    {
        return keccak256(abi.encode(PERMIT_TYPEHASH, owner, spender, value, nonce, deadline));
    }
}
