# Sample Forge verification for POVW

Constructor args must be provided manually.

```
CONSTRUCTOR_ARGS="$(\
    cast abi-encode 'constructor(address,address,bytes32)' \
    "0x8EaB2D97Dfce405A1692a21b3ff3A172d593D319" \
    "0x000006c2A22ff4A44ff1f5d0F2ed65F781F55555" \
    "0x004b225edce73d0fd399993c14bea083a08bc346eacd68a644946179ebc4818f" \
)"
forge verify-contract 0x553ff40b2A36E728CdD79768acb825fb58551bce contracts/src/povw/PovwAccounting.sol:PovwAccounting --constructor-args=${CONSTRUCTOR_ARGS:?} --etherscan-api-key <API_KEY> --watch
```

```
CONSTRUCTOR_ARGS="$(\
    cast abi-encode 'constructor(address,address,bytes32)' \
    "0x8EaB2D97Dfce405A1692a21b3ff3A172d593D319" \
    "0x000006c2A22ff4A44ff1f5d0F2ed65F781F55555" \
    "0x004b225edce73d0fd399993c14bea083a08bc346eacd68a644946179ebc4818f" \
)"
forge verify-contract 0x553ff40b2A36E728CdD79768acb825fb58551bce contracts/src/povw/PovwAccounting.sol:PovwAccounting --constructor-args=${CONSTRUCTOR_ARGS:?} --etherscan-api-key <API_KEY> --watch
```
