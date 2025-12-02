# Boundless BLAKE3 Groth16

Boundless and Risc0 supports the standard Risc0 Groth16 compression which outputs the ReceiptClaim which can be hashed using SHA2 to get the claim digest. This is not ideal for environments which SHA2 hashing may be costly or impossible.

The Chainway Labs team has written a new Groth16 circuit which has a single public output which is the BLAKE3 claim digest of a Risc0 execution.

This repo contains components for creating such proofs and is used in Boundless to enable this type of compression.

We use circom-witnesscalc to do witness generation and either rapidsnark or Risc0's cuda enabled Groth16 prover to generate proofs.

## Setup

Setting up involves downloading `circomlib`, `circom-witnesscalc`, the SRS file, and the circuits which live in a [separate repo](https://github.com/chainwayxyz/risc0-to-bitvm2) maintained by Chainway Labs.
Because we are using circom-witnesscalc, we need to perform a setup to generate the witness graph as well. All of this can be done by running

```
cargo xtask-blake3-groth16 <BLAKE3_GROTH16_SETUP_DIR>
```

### Building Rapidsnark

If using rapidsnark for CPU proving instead of CUDA, you will need to need to manually build rapidsnark. This is all packaged in a dockerfile. To build the rapidsnark dockerfile, run the `scripts/build-docker.sh` from this directory.

## Proving

The prover looks for an environment variable `BLAKE3_GROTH16_SETUP_DIR` which is the directory in which we output the artifacts from the setup step. The Bento docker container will also look for this env variable, so before running a prover, make sure to set that environment variable.

## Proving with Bento

If Bento is used, all of these steps will be handled for you.
