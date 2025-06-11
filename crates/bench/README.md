# Benchmark

## Local Devnet (Dev Mode)

To run a benchmark against a local devnet:

1. Start a local devnet:

   ```bash
   just localnet up
   ```

2. Export the env variables:

```bash
source .env.localnet
```

3. Start a broker

Simplest way is just to run the binary in debug mode

```
RUST_LOG=info cargo run --bin broker
```

4. Define your Benchmark config file. You can find an example in the [data folder](./data/small_test.json).

5. Estimate the benchmark cost (Optional)

   ```bash
   RUST_LOG=boundless_bench=info cargo run --bin boundless-bench -- --bench crates/bench/data/small_test.json --estimate
   ```

6. Run your benchmark:

   ```bash
   RUST_LOG=boundless_bench=info cargo run --bin boundless-bench -- --bench crates/bench/data/small_test.json
   ```

7. Process the results:

   ```bash
   python ./crates/bench/scripts/process.py ./out/bench_1747653790.csv
   ```

> Note: This workflow can be run with `RISC0_DEV_MODE=false` and with a full bento cluster. By default will run localnet in dev mode.

## Testnet

To run a benchmark against a real deployment, just export the correct env variables, then follow the steps of the previous section.
