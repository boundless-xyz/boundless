# Single request fulfillment

This tool is meant to fulfill a single unlocked request on the Boundless Market.
The tool uses the Risc0 zkVM default prover under the hood, as such you can reuse the same env variables
to control what prover backend to use.

> _Note_: It does not run any preflight on the request before attempting at proving it.
> For that, you can use the [boundless-cli](../../../boundless-market/src/bin/boundless-cli.rs) with the `execute` subcommand.

## Usage

To prove a request run:

```bash
RUST_LOG=info,boundless_market=debug target/debug/boundless-fulfill submit --request-id 0x90f79bf6eb2c4f870365e785982e1f101e93b906f272efad
```

You can optionally specify an additional `--tx-hash` containing the calldata of the request.
