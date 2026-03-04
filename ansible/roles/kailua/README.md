# Kailua role

Installs [Kailua](https://github.com/boundless-xyz/kailua) from a GitHub release and runs **`kailua-cli demo`** via a launcher script under systemd.

## What it does

- Downloads `kailua-cli-{{ kailua_version }}-x86_64-unknown-linux-gnu.tar.gz` from GitHub releases and extracts into `kailua_install_dir`.
- Installs a launcher script and a systemd unit that runs it (launcher sets optional env and calls `kailua-cli demo` with configured RPC/data options).

## Requirements

- Linux (Ubuntu/Debian). Network access to GitHub.

## Role variables

| Variable                       | Default                             | Description                                   |
| ------------------------------ | ----------------------------------- | --------------------------------------------- |
| `kailua_install_method`        | `"source"`                          | Must be `"source"` (download release binary). |
| `kailua_version`               | `v1.1.9`                            | Release tag for download.                     |
| `kailua_install_dir`           | `/usr/local/bin`                    | Directory for `kailua-cli` binary.            |
| `kailua_binary_path`           | `/usr/local/bin/kailua-cli`         | Full path to `kailua-cli` binary.             |
| `kailua_install_service`       | `true`                              | Install launcher and systemd unit.            |
| `kailua_launcher_path`         | `/usr/local/bin/kailua_launcher.sh` | Path to the launcher script.                  |
| **Demo (all `kailua_demo_*`)** |                                     |                                               |
| `kailua_demo_blocks_per_proof` | `"10"`                              | `--num-blocks-per-proof`.                     |
| `kailua_demo_l1_rpc`           | `""`                                | `--eth-rpc-url`.                              |
| `kailua_demo_l1_beacon_rpc`    | `""`                                | `--beacon-rpc-url`.                           |
| `kailua_demo_l2_rpc`           | `""`                                | `--op-geth-url`.                              |
| `kailua_demo_op_node_rpc`      | `""`                                | `--op-node-url`.                              |
| `kailua_demo_data_dir`         | `"/kailua-data"`                    | `--data-dir`.                                 |
| `kailua_demo_verbosity`        | `"-vvv"`                            | Verbosity (e.g. `-vvv`).                      |
| **Optional launcher env**      |                                     | Set in inventory.                             |
| `kailua_private_key`           | `""`                                | Exported as `BOUNDLESS_WALLET_KEY`.           |
| `kailua_boundless_rpc_url`     | `""`                                | Exported as `BOUNDLESS_RPC_URL`.              |
| `kailua_order_stream_url`      | `""`                                | Exported as `BOUNDLESS_ORDER_STREAM_URL`.     |
| `kailua_env`                   | (see defaults)                      | Extra env for the process.                    |

### `kailua_env` defaults

The `kailua_env` dict is exported as environment variables by the launcher. Defaults match a
production Kailua prover configuration. Override individual keys in inventory/host\_vars.

| Key                                    | Default            | Description                                         |
| -------------------------------------- | ------------------ | --------------------------------------------------- |
| `NUM_CONCURRENT_PROVERS`               | `4`                | Prover instances running in parallel.               |
| `NUM_CONCURRENT_PREFLIGHTS`            | `4`                | Preflight data fetch threads.                       |
| `NUM_CONCURRENT_PROOFS`                | `64`               | Max pending proofs on the market.                   |
| `NUM_CONCURRENT_WITGENS`               | `10`               | Witness generation threads.                         |
| `NUM_CONCURRENT_R0VM`                  | `10`               | RISC0 Executor instances in parallel.               |
| `NTH_PROOF_TO_PROCESS`                 | `1`                | Process every Nth workload (1 = all).               |
| `MAX_WITNESS_SIZE`                     | `47185920`         | Max input data per proof request (bytes, ~45 MB).   |
| `MAX_BLOCK_DERIVATIONS`                | `180`              | Divide workload into tasks of N blocks.             |
| `MAX_PROOF_STITCHES`                   | `10`               | Max SNARKs recursively verified during composition. |
| `ENABLE_EXPERIMENTAL_WITNESS_ENDPOINT` | `true`             | Bulk witness fetch via `debug_executionWitness`.    |
| `EXPORT_PROFILE_CSV`                   | `true`             | Emit profiling CSV.                                 |
| `CLEAR_CACHE_DATA`                     | `true`             | Clean cache after proof creation.                   |
| `BOUNDLESS_CYCLE_MAX_WEI`              | `60000`            | Max wei per cycle.                                  |
| `BOUNDLESS_CYCLE_MIN_WEI`              | `15000`            | Min wei per cycle.                                  |
| `BOUNDLESS_MEGA_CYCLE_COLLATERAL`      | `5000000000000000` | Collateral per mega-cycle (wei).                    |
| `BOUNDLESS_ORDER_BID_DELAY_FACTOR`     | `0.5`              | Bid delay factor.                                   |
| `BOUNDLESS_ORDER_RAMP_UP_FACTOR`       | `0.75`             | Ramp-up factor.                                     |
| `BOUNDLESS_ORDER_LOCK_TIMEOUT_FACTOR`  | `1.75`             | Lock timeout factor.                                |
| `BOUNDLESS_ORDER_EXPIRY_FACTOR`        | `1.0`              | Order expiry factor.                                |
| `BOUNDLESS_ORDER_SUBMISSION_COOLDOWN`  | `1`                | Cooldown between order submissions (seconds).       |
| `STORAGE_PROVIDER`                     | `s3`               | Storage backend (`s3`).                             |
| `S3_BUCKET`                            | `""`               | S3/R2 bucket name. **Set in inventory.**            |
| `AWS_REGION`                           | `""`               | AWS/R2 region. **Set in inventory.**                |
| `R2_DOMAIN`                            | `""`               | R2 public domain. **Set in inventory.**             |
| `S3_ACCESS_KEY`                        | `""`               | S3/R2 access key. **Set in inventory.**             |
| `S3_SECRET_KEY`                        | `""`               | S3/R2 secret key. **Set in inventory.**             |
| `S3_URL`                               | `""`               | S3/R2 endpoint URL. **Set in inventory.**           |

## Example

```yaml
- hosts: kailua
  become: true
  roles:
    - role: kailua
```

Set RPC URLs and secrets in inventory/group\_vars/host\_vars.
