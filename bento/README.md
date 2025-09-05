# Bento

**Bento** is a high-performance, distributed zero-knowledge proof (ZKP) computation platform built on RISC Zero's zkVM technology. It provides a scalable infrastructure for executing, proving, and verifying computational workloads with cryptographic guarantees.

## 🚀 Overview

Bento is designed to handle large-scale ZKP workloads by distributing computation across multiple worker nodes. It provides:

- **Distributed ZKP Execution**: Execute RISC Zero guest programs across multiple compute nodes
- **Proof Generation**: Generate STARK proofs with configurable segment sizes and cycle limits
- **Proof Composition**: Join multiple proofs into larger, more efficient proofs
- **Proof Verification**: Verify proofs and handle assumption-based verification
- **Scalable Architecture**: Worker-based architecture that can scale horizontally
- **POVW Support**: Optional Proof of Verifiable Work for enhanced security

## 🏗️ Architecture

### Core Components

#### 1. **Workflow Engine** (`crates/workflow`)

The central orchestration service that manages task distribution and execution.

**Key Features:**

- Task polling and distribution
- Worker management and load balancing
- Retry logic and failure handling
- POVW (Proof of Verifiable Work) support
- Background task monitoring

**Worker Types:**

- `exec` - Executes guest programs and generates segments
- `prove` - Generates STARK proofs from segments
- `join` - Composes multiple proofs into larger proofs
- `resolve` - Resolves assumption-based verification
- `finalize` - Finalizes proof verification
- `snark` - Converts STARK proofs to SNARKs
- `keccak` - Handles Keccak hash computations
- `union` - Combines multiple proof types

#### 2. **Task Database** (`crates/taskdb`)

PostgreSQL-based task management system with sophisticated dependency resolution.

**Features:**

- Job and task lifecycle management
- Dependency-based task scheduling
- Retry logic and timeout handling
- Task state tracking (Pending, Ready, Running, Done, Failed)
- Job state management (Running, Done, Failed)

#### 3. **API Service** (`crates/api`)

HTTP API for submitting jobs and managing the Bento cluster.

**Endpoints:**

- Job submission and management
- Image and input upload
- Receipt retrieval and verification
- Cluster status and metrics

#### 4. **Client Library** (`crates/bento-client`)

Rust client library for interacting with Bento services.

**Features:**

- Job submission and monitoring
- Receipt verification
- Batch processing support

#### 5. **Workflow Common** (`crates/workflow-common`)

Shared data structures and constants used across the workflow system.

**Components:**

- Task request/response types
- S3 client for object storage
- Compression type definitions
- Work type constants

#### 6. **Sample Guest** (`crates/sample-guest`)

Example RISC Zero guest programs and methods for testing and development.

**Features:**

- Iterative computation examples
- Composition and Keccak workflows
- Test vectors for validation

## 🔧 Technology Stack

### Core Technologies

- **RISC Zero zkVM**: Zero-knowledge virtual machine for secure computation
- **Rust**: Primary programming language for performance and safety
- **PostgreSQL**: Task database with advanced querying capabilities
- **Redis**: High-performance caching and session storage
- **S3/MinIO**: Object storage for images, inputs, and receipts

### Dependencies

- **sqlx**: Async database toolkit with compile-time query checking
- **tokio**: Async runtime for high-performance I/O
- **serde**: Serialization framework
- **anyhow**: Error handling utilities
- **tracing**: Structured logging and observability
- **bonsai-sdk**: Integration with Bonsai proving service

## 🚀 Getting Started

### Prerequisites

- **Rust**: Latest stable version (1.70+)
- **Docker**: For running PostgreSQL and Redis
- **PostgreSQL**: 13+ (or use Docker)
- **Redis**: 6+ (or use Docker)

### Quick Start with Docker

1. **Clone the repository**
   ```bash
   git clone https://github.com/boundless-xyz/boundless.git
   cd bento
   ```

2. **Start services using Docker**
   ```bash
   ./scripts/docker-setup.sh
   ```

3. **Run full tests (requires database)**
   ```bash
   ./scripts/run_tests.sh
   ```

4. **Clean up Docker services**
   ```bash
   ./scripts/docker-cleanup.sh
   ```

### Manual Setup

1. **Install dependencies**
   ```bash
   cargo install sqlx-cli --no-default-features --features postgres
   ```

2. **Set environment variables**
   ```bash
   export DATABASE_URL="postgres://user:password@localhost:5432/bento"
   export REDIS_URL="redis://localhost:6379"
   export RISC0_DEV_MODE=true
   ```

3. **Run database migrations**
   ```bash
   cd crates/taskdb && sqlx database create && sqlx migrate run
   cd ../broker && sqlx migrate run
   cd ../indexer && sqlx migrate run
   cd ../order-stream && sqlx migrate run
   cd ../slasher && sqlx migrate run
   ```

4. **Build and test**
   ```bash
   cargo build
   cargo test --workspace
   ```

## 📖 Usage

### Running a Workflow Agent

```bash
# Start an executor agent
cargo run -p workflow -- exec --task-stream exec --database-url $DATABASE_URL --redis-url $REDIS_URL

# Start a prover agent
cargo run -p workflow -- prove --task-stream prove --database-url $DATABASE_URL --redis-url $REDIS_URL

# Start a join agent
cargo run -p workflow -- join --task-stream join --database-url $DATABASE_URL --redis-url $REDIS_URL
```

### Submitting Jobs via API

```bash
# Start the API service
cargo run -p api -- --database-url $DATABASE_URL --redis-url $REDIS_URL

# Submit a job (example)
curl -X POST http://localhost:8081/jobs \
  -H "Content-Type: application/json" \
  -d '{
    "image": "base64_encoded_elf",
    "input": "base64_encoded_input",
    "user_id": "user123",
    "assumptions": [],
    "execute_only": false
  }'
```

### Using the Client Library

```rust
use bento_client::Client;

let client = Client::new("http://localhost:8081");
let job_id = client.submit_job(image, input, user_id, assumptions).await?;
let receipt = client.wait_for_receipt(job_id).await?;
```

### Using the Bento CLI

The `bento_cli` provides a command-line interface for submitting jobs and testing the system:

```bash
# Run with iteration count (for testing)
RUST_LOG=info cargo run --bin bento_cli -- -c 32

# Run with custom ELF file and input
RUST_LOG=info cargo run --bin bento_cli -- -f path/to/program.elf -i path/to/input.bin

# Execute only (no proof generation)
RUST_LOG=info cargo run --bin bento_cli -- -e -c 32

# Use custom API endpoint
RUST_LOG=info cargo run --bin bento_cli -- -t http://api.bento.com -c 32
```

**CLI Options:**

- `-f, --elf-file`: Path to RISC Zero ELF file
- `-i, --input-file`: Path to input data file
- `-c, --iter-count`: Iteration count for test vectors
- `-e, --exec-only`: Execute without proof generation
- `-t, --endpoint`: Bento API endpoint (default: http://localhost:8081)

## 🔐 POVW (Proof of Verifiable Work)

Bento supports optional Proof of Verifiable Work for enhanced security:

```bash
# Enable POVW
export POVW_LOG_ID="0x0000000000000000000000000000000000000000"

# Start agents with POVW support
cargo run -p workflow -- join --task-stream join
```

When POVW is enabled:

- Join operations use `join_povw` instead of regular `join`
- Resolve operations use `resolve_povw` instead of regular `resolve`
- Enhanced verification and logging for proof composition

## 🧪 Testing

### Test Types

- **Basic Tests**: No external dependencies, fast execution
- **Integration Tests**: Require PostgreSQL and Redis
- **Unit Tests**: Individual component testing

### Running Tests

```bash
# Full tests (with database)
./scripts/run_tests.sh
```

## 📊 Configuration

### Environment Variables

| Variable         | Description                  | Default                 |
| ---------------- | ---------------------------- | ----------------------- |
| `DATABASE_URL`   | PostgreSQL connection string | Required                |
| `REDIS_URL`      | Redis connection string      | Required                |
| `RISC0_DEV_MODE` | Enable development mode      | `false`                 |
| `POVW_LOG_ID`    | POVW log identifier          | Required to enable POVW |

### Agent Configuration

```bash
# Task stream configuration
--task-stream <stream_type>     # exec, prove, join, resolve, etc.

# Performance tuning
--segment-po2 <size>            # Segment size (default: 20)
--exec-cycle-limit <cycles>     # Execution cycle limit (default: 100M)
--poll-time <seconds>           # Polling interval (default: 1)

# Retry and timeout settings
--prove-retries <count>         # Prove retry attempts (default: 3)
--prove-timeout <minutes>       # Prove timeout (default: 30)
--join-retries <count>          # Join retry attempts (default: 3)
--join-timeout <minutes>        # Join timeout (default: 10)

# Database and Redis
--db-max-connections <count>    # Database connection pool size (default: 1)
--redis-ttl <seconds>          # Redis TTL (default: 8 hours)
```

## 🏭 Production Deployment

### Scaling Considerations

- **Horizontal Scaling**: Run multiple agents of each type
- **Load Balancing**: Distribute tasks across multiple workers
- **Database Scaling**: Use connection pooling and read replicas
- **Redis Clustering**: For high-availability caching
- **Object Storage**: S3-compatible storage for large files

### Monitoring and Observability

- **Structured Logging**: Uses `tracing` for comprehensive logging
- **Metrics**: Task completion rates, processing times, error rates
- **Health Checks**: Database and Redis connectivity monitoring
- **Alerting**: Task failure and timeout notifications

### Security

- **API Key Authentication**: Required for job submission
- **Network Isolation**: Separate worker and API networks
- **Input Validation**: Comprehensive input sanitization
- **Resource Limits**: Configurable execution limits

## 🔧 Development

### Project Structure

```
bento/
├── crates/
│   ├── workflow/           # Main workflow engine
│   ├── workflow-common/    # Shared types and utilities
│   ├── taskdb/            # Task database management
│   ├── api/               # HTTP API service
│   ├── bento-client/      # Client library
│   └── sample-guest/      # Example guest programs
├── scripts/               # Development and deployment scripts
├── target/                # Build artifacts
└── Cargo.toml            # Workspace configuration
```

### Adding New Task Types

1. **Define the task type** in `workflow-common/src/lib.rs`
2. **Implement the task handler** in `workflow/src/tasks/`
3. **Add routing logic** in `workflow/src/lib.rs`
4. **Update tests** and documentation

### Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Add tests for new functionality
5. Ensure all tests pass
6. Submit a pull request

## 📚 API Reference

### Job Management

- `POST /jobs` - Submit a new job
- `GET /jobs/{job_id}` - Get job status
- `GET /jobs/{job_id}/receipt` - Get job receipt
- `DELETE /jobs/{job_id}` - Cancel a job

### Image Management

- `POST /images` - Upload a new image
- `GET /images/{image_id}` - Get image information
- `DELETE /images/{image_id}` - Delete an image

### Receipt Management

- `GET /receipts` - List available receipts
- `GET /receipts/{receipt_id}` - Download a receipt
- `POST /receipts/verify` - Verify a receipt

### Work Receipts Management

- `GET /work-receipts` - List all work receipts with POVW metadata
- `GET /work-receipts/{receipt_id}` - Download a specific work receipt

**POVW Metadata**: Each work receipt includes Proof of Verifiable Work (POVW) information:

- `povw_log_id`: The POVW log identifier for tracking receipt provenance
- `povw_job_number`: The POVW job number for client-side deduplication
- Metadata is stored alongside receipts in `{receipt_id}_metadata.json` files

## 🤝 Community

- **GitHub**: [https://github.com/boundless-xyz/boundless](https://github.com/boundless-xyz/boundless)
- **Discussions**: GitHub Discussions for questions and ideas
- **Issues**: Bug reports and feature requests
- **Contributing**: See CONTRIBUTING.md for development guidelines

## 📄 License

This project is licensed under the Business Source License (BSL). See the [LICENSE-BSL](LICENSE-BSL) file for details.

## 🙏 Acknowledgments

- **RISC Zero**: For the foundational zkVM technology
- **Contributors**: All the developers who have contributed to Bento
- **Open Source Community**: For the excellent tools and libraries used

---

**Bento** - Scaling zero-knowledge proofs to new heights 🚀
