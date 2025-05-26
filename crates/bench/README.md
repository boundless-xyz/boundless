# Benchmark

## Local Devnet (Dev Mode)

To run a benchmark against a local devnet:

1. Start a local devnet:

   ```bash
   just localnet up
   ```

2. Export the env variables:

   ```bash
   source <(just env localnet)
   ```

   > _Note_: After sourcing, you may want to unset the `ORDER_STREAM_URL` if you wish to submit benchmark request onchain.

3. Define your Benchmark config file. You can find an example in the [data folder](./data/small_test.json).
4. Estimate the benchmark cost (Optional)

   ```bash
   RUST_LOG=boundless_bench=info cargo run --bin boundless-bench -- --bench crates/bench/data/small_test.json --estimate
   ```

5. Run your benchmark:

   ```bash
   RUST_LOG=boundless_bench=info cargo run --bin boundless-bench -- --bench crates/bench/data/small_test.json
   ```

6. Process the results:

   ```bash
   python ./crates/bench/scripts/process.py ./out/bench_1747653790.csv
   ```

## Testnet

To run a benchmark against a real deployment, just export the correct env variables, then follow the steps of the previous section.
