# Docker Images Runbook

How Boundless Docker images are built, released, tagged, and deployed.

## Image Inventory

| Image            | Contents                                             | Compose service(s)             | Override env var    |
| ---------------- | ---------------------------------------------------- | ------------------------------ | ------------------- |
| `bento-agent`    | GPU proving agent (CUDA build, runs on NVIDIA GPUs)  | `gpu_prove_agent`              | `AGENT_IMAGE`       |
| `bento-agent-cpu`| CPU agent for executor and auxiliary workers          | `exec_agent`, aux workers      | `CPU_AGENT_IMAGE`   |
| `bento-rest-api` | REST API for task submission and status               | `rest_api`                     | `REST_API_IMAGE`    |
| `bento-cli`      | CLI tools; also used as the `miner` service          | `miner`                        | `BENTO_CLI_IMAGE`   |
| `broker`         | Broker binary for order picking and fulfillment      | `broker`                       | `BROKER_IMAGE`      |

Five additional infra images are built but not part of the prover compose stack: `slasher`, `order-stream`, `indexer`, `distributor`, `order-generator`.

## CI Pipeline (`docker-services.yml`)

**Triggers**: push to `main`, tags matching `bento-v*` or `broker-v*`, and `workflow_dispatch`.

**What it does**:

1. Builds 10 services in parallel:
   - 5 infra services (slasher, order-stream, indexer, distributor, order-generator)
   - broker
   - bento-agent (GPU, built on a CUDA runner)
   - bento-agent-cpu
   - bento-rest-api
   - bento-cli
2. Tags each image with `nightly-<commit-sha>` and `nightly-latest`.

After a push to `main`, images are available within ~15-20 minutes.

## Release Process

### Bento release (`bento-release.yml`)

Triggered by a `bento-v*` tag (e.g. `bento-v1.3.0`).

1. Waits for `docker-services.yml` to finish building the `nightly-<sha>` images.
2. Extracts binaries from the `nightly-<sha>` images via `docker cp`.
3. Creates a GitHub Release with tarballs (`bento-agent`, `bento-rest-api`, `bento-cli`).
4. Retags images with version tags using `crane`:
   - `bento-v1.3.0` (exact version)
   - `bento-v1.3` (minor, skipped for RC)
   - `latest` (skipped for RC)

### Broker release (`broker-release.yml`)

Same pattern, triggered by a `broker-v*` tag. Extracts the broker binary, creates a GitHub Release, and retags the broker image.

### RC vs Full Release

Tags containing `-rc.` (e.g. `bento-v1.3.0-rc.1`) are release candidates:

- The exact version tag is always applied (`bento-v1.3.0-rc.1`).
- The `latest` tag is **not** applied.
- The minor version tag (e.g. `bento-v1.3`) is **not** applied.

This prevents RC images from becoming the default pull for production hosts.

## Manual Retag (`retag-image.yml`)

A `workflow_dispatch` workflow to retag any image without rebuilding.

Inputs:
- Source tag (e.g. `nightly-abc1234`)
- Target tag (e.g. `bento-v1.3.1`)
- Image name(s)

Useful for promoting a known-good nightly image to a release tag, or fixing a missed tag.

## Compose Defaults and Overrides

`compose.yml` defaults (when no env vars are set):

| Service            | Default image tag |
| ------------------ | ----------------- |
| GPU agents         | `bento-agent:latest` |
| CPU agents         | `bento-agent-cpu:latest` |
| REST API           | `bento-rest-api:latest` |
| Miner              | `bento-cli:latest` |
| Broker             | `broker:latest` |

Override any image via the `.env` file or environment variables:

```bash
AGENT_IMAGE=ghcr.io/boundless-xyz/boundless/bento-agent:nightly-latest
CPU_AGENT_IMAGE=ghcr.io/boundless-xyz/boundless/bento-agent-cpu:nightly-latest
REST_API_IMAGE=ghcr.io/boundless-xyz/boundless/bento-rest-api:nightly-latest
BENTO_CLI_IMAGE=ghcr.io/boundless-xyz/boundless/bento-cli:nightly-latest
BROKER_IMAGE=ghcr.io/boundless-xyz/boundless/broker:nightly-latest
```

## Switching Staging to Nightly Images

To move a staging host from build-from-source to nightly images, update the Ansible inventory:

```yaml
prover_84532_staging_nightly:
  hosts:
    dev-prover:
      prover_version: main
      prover_image_tag: "nightly-latest"
      prover_boundless_build: ""          # empty = don't build from source
      prover_wait_for_nightly_image: true  # wait for CI images before deploy
```

Key points:
- `prover_image_tag` sets every image to the same tag (e.g. `nightly-latest`).
- `prover_boundless_build` must be empty (not `"all"`), otherwise images are built locally.
- `prover_wait_for_nightly_image` prevents the deploy from restarting services before CI has pushed the images for the deployed commit.

## Troubleshooting

### `SIGILL` (illegal instruction) at startup

Older images were built with `RUSTFLAGS="-C target-cpu=native"`, which embedded host-specific CPU instructions. This was removed. If you hit SIGILL, pull a newer image:

```bash
docker compose pull
docker compose up -d
```

### Missing `r0vm` binary

The `r0vm` binary is bundled inside the agent images. If it is missing, the image may be stale or corrupted. Re-pull:

```bash
docker compose pull bento-agent bento-agent-cpu
```

### Image not found / pull errors

1. Verify the tag exists:
   ```bash
   crane ls ghcr.io/boundless-xyz/boundless/bento-agent | grep nightly-latest
   ```
2. If deploying with `prover_wait_for_nightly_image: true`, check that the `docker-services` workflow succeeded for the target commit.
3. For release tags, confirm the release workflow completed and retagged correctly.

### Checking which commit an image was built from

```bash
# The nightly-<sha> tag encodes the commit directly:
docker inspect ghcr.io/boundless-xyz/boundless/bento-agent:nightly-latest \
  --format '{{ index .Config.Labels "org.opencontainers.image.revision" }}'
```
