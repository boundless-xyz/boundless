# Contracts verification examples

## Sample Forge verification for POVW

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

## Sample Forge verification for Boundless Market

Constructor args must be provided manually.

```
CONSTRUCTOR_ARGS="$(\
    cast abi-encode 'constructor(address,bytes32,bytes32,uint32,address)' \
    "0x0b144e07a0826182b6b59788c34b32bfa86fb711" \
    "0x03831182a226b5f5a4d358704a9f9d0bcd4dc48e6e577dc7db84d94892024938" \
    "0x0000000000000000000000000000000000000000000000000000000000000000" \
    "0x0000000000000000000000000000000000000000000000000000000000000000" \
    "0xAA61bB7777bD01B684347961918f1E07fBbCe7CF" \
)"
forge verify-contract 0x8d3D36400d0a8Cf7cF217D28D366a0189F0850B6 contracts/src/BoundlessMarket.sol:BoundlessMarket --constructor-args=${CONSTRUCTOR_ARGS:?} --rpc-url $RPC_URL --etherscan-api-key <API_KEY> --watch
```
