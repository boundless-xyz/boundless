# Guest Programs: Understanding What You're Proving

## What is a Guest Program?

In the RISC Zero zkVM, a **guest program** is code that runs inside the zero-knowledge virtual machine. It executes like normal software, but produces a cryptographic proof that it ran correctly. Anyone can verify this proof without re-executing the program.

The architecture has two sides:

- **Host** — your application code running on a normal computer. It provides inputs to the guest and submits the proof request.
- **Guest** — code compiled to RISC-V that runs inside the zkVM. It reads inputs, performs computation, and writes outputs to a public **journal**.

The guest program is compiled to a RISC-V ELF binary. This binary has a deterministic **image ID** — a cryptographic hash of the program. The image ID is what gets verified on-chain: it proves that *this specific program* produced the output.

## A Minimal Guest Program

Here's the simplest possible guest program. It reads stdin and writes it unchanged to the journal:

```rust
use std::io::Read;
use risc0_zkvm::guest::env;

pub fn main() {
    // Read the entire input stream as raw bytes.
    let mut message = Vec::<u8>::new();
    env::stdin().read_to_end(&mut message).unwrap();

    // Commit exactly what the host provided to the journal.
    env::commit_slice(message.as_slice());
}
```

That's it — ~6 lines of logic. Here's what each part does:

1. `env::stdin()` — reads the input bytes that the host (requestor) provided.
2. `env::commit_slice()` — writes data to the **journal**, which is the public output of the proof. Anything committed to the journal is visible to verifiers.

The proof statement this creates is: *"I ran this program (identified by its image ID) with some input, and it produced this journal output."*

Real guest programs do more interesting things — validate signatures, check Merkle proofs, run ML inference — but the structure is always the same: read input, compute, commit output.

## The Boundless Proof Lifecycle

Every proof request on Boundless follows the same flow, regardless of guest program complexity:

1. **You submit a request** with a program URL and input data
2. **A prover picks it up** from the Boundless market
3. **The prover executes** the guest inside the zkVM and generates a proof
4. **The proof is verified** on-chain by the Boundless market contract
5. **You receive** the journal (output) and seal (proof)

This is the same flow used for complex programs — fraud proofs, identity verification, private computation. Simple programs just make the flow easy to understand.

## Journal vs. Seal

When your proof request is fulfilled, you get two things:

- **Journal** — the public output bytes committed by `env::commit_slice()`. The journal is what your application logic cares about.
- **Seal** — the cryptographic proof that the computation ran correctly. This is what smart contracts verify on-chain. You don't need to inspect it directly.

Together, the journal and seal say: *"This program with image ID X produced output Y, and here's the cryptographic proof."*

## Using the Boundless SDK

The counter example from the Boundless repo shows how the SDK submits a proof request programmatically:

```rust
use boundless_market::Client;

// Build the Boundless client
let client = Client::builder()
    .with_rpc_url(rpc_url)
    .with_private_key(private_key)
    .build()
    .await?;

// Create and submit a proof request
let request = client.new_request()
    .with_program_url(program_url)?
    .with_stdin(&input_bytes);

let (request_id, expires_at) = client.submit(request).await?;

// Wait for fulfillment
let fulfillment = client
    .wait_for_request_fulfillment(
        request_id,
        Duration::from_secs(5),
        expires_at,
    )
    .await?;
```

The CLI's `submit` command does the same thing under the hood — it takes a `--program-url` pointing to a pre-built ELF on IPFS and `--input-file` for the stdin bytes.

## Building a Guest Program Locally (Advanced)

If you want to build a guest program yourself:

```bash
# Clone the Boundless repo
git clone --depth 1 https://github.com/boundless-xyz/boundless.git
cd boundless

# Install the RISC Zero toolchain
rzup install

# Build a guest program (e.g., the echo guest)
cargo build -p echo --target riscv32im-risc0-zkvm-elf --release

# The ELF binary is at:
# target/riscv32im-risc0-zkvm-elf/release/echo
```

The resulting ELF binary can be uploaded to any public URL (IPFS, S3, etc.) and used with `--program-url`.

## Further Reading

- [Boundless Proof Lifecycle](https://docs.boundless.network/developers/proof-lifecycle)
- [RISC Zero zkVM Guest Documentation](https://dev.risczero.com/api/zkvm/guest)
- [Boundless SDK Examples](https://github.com/boundless-xyz/boundless/tree/main/examples)
