# Prover

Distributed zero-knowledge proof computation platform built on RISC Zero's zkVM. Breaks ZKP workloads into parallelizable tasks (execute, prove, join, resolve, snark) and distributes them across CPU and GPU worker nodes coordinated through Redis.

## Architecture

```
                 ┌─────────────┐
  Broker ──────> │  REST API   │ <────── bento_cli
                 │  (port 8081)│
                 └──────┬──────┘
                        │
                 ┌──────┴──────┐
                 │    Redis    │
                 │  (taskdb)   │
                 └──────┬──────┘
                        │
       ┌────────────────┼────────────────┐
       │                │                │
┌──────┴──────┐  ┌─────┴──────┐  ┌──────┴──────┐
│ exec agents │  │prove agents│  │  aux agents  │
│   (CPU)     │  │   (GPU)    │  │   (CPU)      │
└─────────────┘  └────────────┘  └──────────────┘
```

**REST API** (`crates/api`) -- HTTP service that accepts job submissions, manages image/input uploads, orchestrates the task DAG in Redis, and serves results. Also exposes internal worker endpoints that GPU agents use to claim tasks and report results.

**Workflow Agent** (`crates/workflow`) -- Single binary that runs in one of several modes depending on `--task-stream`:

- `exec` -- Executes guest programs, produces segments. CPU-bound. Reads/writes directly to Redis.
- `prove` -- Proves individual segments (STARK). GPU-bound. Claims work via the REST API.
- `join` -- Joins adjacent proofs into larger proofs. GPU-bound.
- `snark` -- Converts STARK proofs to Groth16/BLAKE3-Groth16 SNARKs. GPU-bound.
- `aux` -- Housekeeping: requeues timed-out tasks, cleans up completed jobs, fixes stuck dependencies.
- `keccak` -- Keccak coprocessor proofs. GPU-bound.

GPU workers (`prove`, `join`, `snark`, `keccak`) operate statelessly through the API -- they don't need direct Redis access. CPU workers (`exec`, `aux`) connect to Redis directly.

**TaskDB** (`crates/taskdb`) -- Redis-backed task scheduler with dependency resolution, priority queues (high/medium/low), blocking work claims with push wakeups, retry tracking, and timeout management.

**Workflow Common** (`crates/workflow-common`) -- Shared types (task definitions, request/response structs), local-disk object storage client, and Prometheus metrics helpers.

**Bento Client** (`crates/bento-client`) -- CLI tool for submitting test jobs. Useful for benchmarking and smoke tests.

**Sample Guest** (`crates/sample-guest`) -- Example RISC Zero guest programs with configurable iteration counts for testing.

## Proof Pipeline

A job submission triggers this task DAG:

1. **Execute** -- Run the guest program, produce N segments
2. **Prove** -- Prove each segment independently (N parallel tasks)
3. **Join** -- Binary-tree reduction of proofs (log2(N) levels)
4. **Resolve** -- Resolve assumption-based verification
5. **Finalize** -- Produce the final composite receipt
6. **Snark** (optional) -- Compress to Groth16 or BLAKE3-Groth16

When POVW is enabled (`POVW_LOG_ID` env var), join and resolve use POVW-aware variants that track proof provenance.

## Running with Docker Compose

The recommended deployment uses `prover-compose.yml` from the repo root:

```bash
# Start the full stack (Redis + API + agents + monitoring)
docker compose -f prover-compose.yml up -d

# With broker profile
docker compose -f prover-compose.yml --profile broker up -d

# Scale executor agents
BENTO_EXECUTOR_COUNT=8 docker compose -f prover-compose.yml up -d
```

The compose file automatically detects all GPUs via `nvidia-smi` and spawns one prove agent per GPU.

## Development Setup

```bash
cd prover

# Start Redis for tests
./scripts/docker-setup.sh

# Build
cargo build --workspace

# Run tests
./scripts/run_tests.sh

# Run specific crate tests
./scripts/run_tests.sh taskdb
./scripts/run_tests.sh workflow

# Tear down
./scripts/docker-cleanup.sh
```

### Running Agents Locally

```bash
export REDIS_URL="redis://localhost:6379"
export RISC0_DEV_MODE=true

# Start the API
cargo run -p api --bin rest_api -- --bind-addr 0.0.0.0:8081

# Start an executor (in another terminal)
cargo run -p workflow --bin agent -- -t exec

# Start a prover (in another terminal, needs GPU)
cargo run -p workflow --bin agent -- -t prove

# Submit a test job
cargo run -p bento-client --bin bento_cli -- --iter-count 32
```

## Configuration

### Environment Variables

| Variable         | Description                      | Default                  |
| ---------------- | -------------------------------- | ------------------------ |
| `REDIS_URL`      | Redis connection string          | Required for CPU workers |
| `BENTO_API_URL`  | REST API base URL                | `http://localhost:8081`  |
| `RISC0_DEV_MODE` | Skip real proving (fast, no GPU) | `false`                  |
| `POVW_LOG_ID`    | Enable Proof of Verifiable Work  | Disabled when unset      |
| `RUST_LOG`       | Log level filter                 | `info`                   |

### Agent CLI Flags

```
-t, --task-stream <type>       Worker type: exec, prove, join, snark, aux, keccak
-s, --segment-po2 <size>       Segment size power-of-2 (default: 20)
-e, --exec-cycle-limit <M>     Max execution cycles in millions (default: 100000)
    --redis-ttl <secs>         Object expiry in Redis (default: 28800 / 8h)
    --prove-retries <n>        Max prove retry attempts (default: 3)
    --prove-timeout <min>      Prove timeout in minutes (default: 30)
    --monitor-requeue          Enable background retry/requeue monitoring
```

## Project Structure

```
prover/
├── crates/
│   ├── api/               # REST API (axum)
│   ├── workflow/           # Agent binary and task handlers
│   ├── workflow-common/    # Shared types, storage, metrics
│   ├── taskdb/             # Redis task scheduler
│   ├── bento-client/       # CLI test tool
│   └── sample-guest/       # Example guest programs
├── scripts/                # Docker setup, test runner
└── Cargo.toml              # Workspace root
```

## License

Business Source License (BSL). See [LICENSE-BSL](LICENSE-BSL).
