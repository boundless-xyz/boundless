<p align="center">
  <img src="Boundless_Logo black.png" alt="Boundless Logo" width="200">
</p>

# Boundless

This repository contains the core primitives for Boundless.

> **Note:** If you are a builder looking to build an application on Boundless, you should start with the [Boundless Foundry Template](https://github.com/boundless-xyz/boundless-foundry-template) and the [Boundless Builder Docs](https://docs.beboundless.xyz/build/build-a-program).

> **Note:** If you are a prover looking to get started, please refer to the [Boundless Prover Quick Start Guide](https://docs.beboundless.xyz/prove/quick-start).

## Repository Structure

The repository is structured as a monorepo and contains Rust crates and Solidity contracts. Some key components:

- **Boundless Core Contracts**: The core smart contracts for Boundless. [./contracts](./contracts)
- **Boundless SDK**: Rust SDK for interacting with Boundless. [./crates/boundless-market](./crates/boundless-market)
- **Boundless CLI**: Command-line interface for interacting with Boundless. [./crates/boundless-cli](./crates/boundless-cli)
- **Boundless Broker**: Our sample prover implementation. [./crates/broker](./crates/broker)
- **Boundless zkVM Guests**: The zkVM guests required for generating proofs on Boundless. [./crates/guest](./crates/guest) and [./crates/assessor](./crates/assessor)

## Developing

If you don't already have Rust installed, start by [installing Rust and rustup](https://doc.rust-lang.org/cargo/getting-started/installation.html).

Then download the RISC Zero toolchain and install it using rzup:

```sh
curl -L https://risczero.com/install | bash
```

Next we can install the RISC Zero toolchain by running rzup install:

```sh
rzup install
```

You can verify the installation was successful by running:

```sh
cargo risczero --version
```

If you don't already have Forge installed, you can install it using Foundry:

```sh
curl -L https://foundry.paradigm.xyz | bash
foundryup
```

To build the Solidity contracts, run:

```sh
forge build
```

To build the Rust crates, run:

```sh
cargo build
```

## Documentation

You can find the documentation in the [documentation](./documentation) folder.

To build it and serve it locally, run the following commands:

```sh
bun install
bun run docs
```

Then open your browser and navigate to `http://localhost:5173`.

## License

Rust SDK including the `boundless-market` crate is licensed under the Apache-2.0 license.
See [LICENSE-Apache-2.0][./LICENSE-Apache-2.0] for a copy of the license.

Other source code in this repository is licensed under the Bussiness Source License, with conversion to Apache-2.0 at a future date.
See [LICENSE-BSL](./LICENSE-BSL) for a copy of the license.
