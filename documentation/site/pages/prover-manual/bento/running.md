---
title: Running Bento
description: Bento is a Docker compose stack containing all the services to run a Bento cluster on a single or many machines.
---

# Running Bento

Bento is a Docker compose stack containing all the services to run a Bento cluster on a single or many machines.
It includes Docker build files for internal services as well as external images for `postgres/redis/grafana/minio` for support services.

## Dependencies

- [Docker compose](https://docs.docker.com/compose)
- Docker Nvidia support

## Configuration

The `compose.yml` file defines all the services within the Bento. Here you can configure the number of GPU's via adding new `gpu_agent<I>` and changing the `device_ids` to map to specific physical GPUs (if on a single host, multi-GPU config).

Under the `exec_agent` service you can configure the segment size with the `--segment-po2` flag. Bigger segments are preferable for performance but do impact the proving systems conjectured security bits slightly. In order to pick the right segment po2 for your GPU VRAM see [reports.risczero.com](https://reports.risczero.com) for details.

Services can be run on other hosts, as long as the IP addresses for things link PostgreSQL / Redis / MinIO are updated on the remote host. See the `.env-compose` \*\_HOST configuration options here to adjust them.

:::warning[Warning]
TODO: Write more here about how to do multi-host services like gpu-agent / exec-agent
:::

## Host Setup

At this time Ubuntu is the only supported Operating system. Other operating systems _should_ work, but driver support (host Nvidia drivers matching container drivers etc.), compile dependencies, and testing fall to the user to complete.

For a quick set up of Boundless host dependencies on Ubuntu 22.04 LTS, please run:

```sh [Terminal]
scripts/setup.sh
```

## Running

To build and spin up a Bento cluster locally using Docker:

```sh [Terminal]
docker compose --env-file ./.env-compose up --build
```

Optionally you can use the startup script included in this repo:

```sh [Terminal]
scripts/boundless_service.sh start
```

To stop the Boundless service:

```sh [Terminal]
scripts/boundless_service.sh stop
```

## Sending a Sample Proof to the Cluster

Using a simple test vector for testing different cycle counts (via the -c flag):

```sh [Terminal]
RUST_LOG=info cargo run --bin bento_cli -- -c 32
```

Or with a existing elf / input file:

```sh [Terminal]
RUST_LOG=info cargo run --bin bento_cli -- -f ./crates/bento-client/method_name -i /tmp/input.bin
```
