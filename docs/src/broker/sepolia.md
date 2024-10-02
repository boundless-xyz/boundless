<div class="warning">
THIS IS OUT OF DATE
</div>

# Playing with the Contracts Deployed on Sepolia

Ensure the following software is installed on your machine before proceeding:

- **[Rust](https://www.rust-lang.org/tools/install)** (version 1.79 or higher)
- **[Foundry](https://book.getfoundry.sh/getting-started/installation)** (version 0.2 or higher)
- **[jq](https://jqlang.github.io/jq/download/)**
- **[MetaMask](https://metamask.io/download/)**

## Contract addresses

### Sepolia

| Contract Name            | Contract Address                                                                                                              |
| ------------------------ | ----------------------------------------------------------------------------------------------------------------------------- |
| `ProofMarket`            | 0x1074Dc9CaEa49B5830D3b9A625CdEA9C1038FC45 <!-- TODO link to contract -->                                                     |
| `SetVerifier`            | 0x6a89661977CBd8825dDB8F0b2429eBf773444dFa <!-- TODO link to contract -->                                                     |
| `RiscZeroVerifierRouter` | [0x925d8331ddc0a1F0d96E68CF073DFE1d92b69187](https://sepolia.etherscan.io/address/0x925d8331ddc0a1F0d96E68CF073DFE1d92b69187) |
| `Counter` example        | 0xC7f3135fBC0Aeca1Fd2cFB04319996efea53Eb7a <!-- TODO link to contract -->                                                     |

### L1 Sepolia

| Contract Name           | Contract Address                                                                                                              |
| ----------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| `CrossDomainSetOfTruth` | [0x0842dcBE1fcE940b17d3aE10f824e264107a0446](https://sepolia.etherscan.io/address/0x0842dcBE1fcE940b17d3aE10f824e264107a0446) |

## Getting Started

#### Ensure Sufficient Funding

- To interact with the Proof Market and request proof, you should have at least 0.5 Sepolia ETH.
- To act as a broker or prover, you should also have at least 0.5 Sepolia ETH.
- Specify the account private key as an environment variable or command line argument. To get the private key of the account holding your funds in MetaMask, go to "Account details" > "Show private key" and copy the key. Ensure the key starts with `0x`; if not, prepend `0x` when supplying the key.

#### Use an RPC Provider

You need an RPC provider to interact with the network. [Alchemy](https://www.alchemy.com/) supports Sepolia, so creating a free account there is recommended. Set the following environment variables according to your Alchemy URLs:

```bash
export RPC_URL="<ALCHEMY-SEP-URL>"
```

#### Use IPFS Storage

IPFS storage is supported through [Pinata](https://www.pinata.cloud/), which offers a free tier sufficient for this use case. To use Pinata, fetch the JWT credentials and set the `PINATA_JWT` environment variable.

### Submit Your First Offer

Ensure the environment variables `RPC_URL` and `PINATA_JWT` are set. Run the following command to submit an offer to prove the [ECHO](https://github.com/boundless-xyz/boundless/blob/main/crates/guest/echo/echo/src/main.rs) of `Hello world!`. The image is uploaded to IPFS using Pinata, and the input is supplied in the calldata of the transaction (`--inline-input`). No additional journal constraints are checked on-chain (`--journal-prefix`). The offer uses the example [`offer.yaml`](https://github.com/boundless-xyz/boundless/blob/main/offer.yaml).

```bash
RUST_LOG=info cargo run --bin cli -- --private-key <METAMASK_PRIVATE_KEY> --proof-market-address 0x1074Dc9CaEa49B5830D3b9A625CdEA9C1038FC45 submit-offer --input "Hello world!" --inline-input --encode-input --journal-prefix "" offer.yaml
```

See [Submit proving requests](../market/proving_request.md) for more info on request.
