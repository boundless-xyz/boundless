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

## Example

```yaml
- hosts: kailua
  become: true
  roles:
    - role: kailua
```

Set RPC URLs and secrets in inventory/group\_vars/host\_vars.
