# Troubleshooting — Boundless Echo Proof

## Common Errors

| Error | Cause | Fix |
|-------|-------|-----|
| `insufficient funds` | Wallet ETH balance too low for gas + deposit | Fund wallet on Base via bridge.base.org or CEX withdrawal |
| `execution reverted` on deposit | Trying to deposit more ETH than wallet holds | Check balance with `cast balance <addr> --rpc-url <rpc>` |
| `RequestHasExpired` | Proof request timed out before a prover fulfilled it | Increase `--timeout` and `--max-price` to attract provers |
| `program too large` | ELF binary exceeds 50MB limit | Ensure you're using the correct echo program URL |
| `connection refused` / `transport error` | RPC endpoint unreachable | Verify `REQUESTOR_RPC_URL` is correct and the provider is online |
| `invalid Pinata JWT` | Bad or expired Pinata token | Regenerate JWT at app.pinata.cloud/keys |
| `no provers available` | No provers currently serving requests | Wait and retry — prover availability fluctuates. Increase `--max-price` |
| `Failed to build Boundless Client` | Config missing or invalid | Run `boundless requestor config` to check, or re-run `boundless requestor setup` |
| `Failed to parse request from YAML` | Malformed YAML in submit-file | Validate YAML syntax; check field names match expected schema |
| `Invalid imageUrl` | Malformed program URL in YAML | Ensure URL is a valid HTTP/HTTPS URL (IPFS gateway URLs work) |
| `Malformed ProgramBinary` | Program ELF was built against a different risc0-zkvm version than the installed CLI | Use `scripts/discover-programs.sh` to find a recently fulfilled program with known-working inputs |

## Installation Issues

### `cargo install` fails for boundless-cli

**Symptom:** Compilation errors or dependency resolution failures.

**Fixes:**
- Ensure Rust is up to date: `rustup update`
- Use the exact install command with `--locked`: `cargo install --locked --git https://github.com/boundless-xyz/boundless boundless-cli --branch release-1.2 --bin boundless`
- If it still fails, try clearing the cargo cache: `cargo cache -r all` or `rm -rf ~/.cargo/registry/cache`

### `rzup install` hangs or fails

**Symptom:** RISC Zero toolchain installation doesn't complete.

**Fixes:**
- Check internet connectivity
- Try `rzup install --verbose` for detailed output
- Ensure you have enough disk space (~2GB for the full toolchain)
- On Linux, ensure `build-essential` / `gcc` is installed

### Foundry (`cast`) not found after install

**Symptom:** `foundryup` completed but `cast` isn't in PATH.

**Fix:** Add to your shell profile:
```bash
export PATH="$HOME/.foundry/bin:$PATH"
```

## Request Lifecycle Issues

### Request stuck — no prover picking it up

The Boundless market is an auction. If your offer price is too low, provers may skip it.

**Fixes:**
- Increase `--max-price` (try `2000000000000000` = 0.002 ETH)
- Increase `--ramp-up-period` to give the price more time to rise
- Check if the network has active provers by looking at recent fulfillments on the Base block explorer

### Request fulfilled but journal is empty

The echo program commits whatever stdin it receives. If the journal is empty, the input was empty.

**Fix:** Ensure `--input` has a non-empty value, or check the `data` field in your YAML.

### `--wait` times out

The `--wait` flag polls every 5 seconds until expiration. If the request expires before fulfillment:

- The request was not attractive enough for provers — increase price
- Network congestion — try `--offchain` for faster matching
- Try again with a longer `--timeout`

## Config Reset

To start fresh with the requestor configuration:

```bash
rm -rf ~/.boundless/
boundless requestor setup
```

This removes all stored config and re-runs the interactive wizard.

## Getting Help

- [Boundless Docs](https://docs.boundless.network)
- [Boundless GitHub Discussions](https://github.com/boundless-xyz/boundless/discussions)
- [RISC Zero Discord](https://discord.gg/risczero)
