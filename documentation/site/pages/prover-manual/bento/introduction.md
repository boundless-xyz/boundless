---
title: Bento Technical Design
description: Bento is designed to be a horizontally scalable, semi multi-tenant proving cluster for the RISC0 zkVM.
---

# Bento Technical Design

Bento is designed to be a horizontally scalable, semi multi-tenant proving cluster for the [RISC0 zkVM](https://risczero.com).
Some core features of Bento include:

- Clusters of arbitrarily sized proving capacity (read GPUs).
- Support for arbitrarily sized proofs.
- Durable storage and caching for receipts (stark and snark), images and inputs.
- Robust retry system.
- API for proof management.

## Minimum Specs

Bento can run on a single machine with a single GPU with as low as 4GB of VRAM, but this would only be recommended for testing purposes. Below is a minimum configuration for reasonable proving performance:

- CPU - 16 threads, reasonable single core boost performance (>3Ghz)
- Memory - 32 GB
- Disk - 200 GB of solid state storage, NVME / SSD preferred
- GPU - NVIDIA RTX 3080 or T4, minimum VRAM 8GB [PO2 relationship to VRAM](#more-on-the-gpu)

## Internals

### Core Infrastructure

Bento's infrastructure is composed of a few core open source projects:

- [Docker](https://docs.docker.com/get-started/docker-overview)
- [PostgreSQL](https://www.postgresql.org)
- [Redis](https://redis.io)
- [MinIO](https://min.io)
- [Grafana](https://grafana.com) _(optional for monitoring)_

### Bento Components

The below components are built on top of the core infrastructure. These components are the basis for Bento and are critical
in its operation:

- API
- TaskDB
- CPU (executor) Agent
- GPU (prover) Agent
- Aux Agent
- [Broker][page-broker]

## Design

Bento's design philosophy is centered around TaskDB. TaskDB is a database schema in PostgreSQL that acts as a central communications hub, scheduler, and queue for all of the Bento system. To illustrate further, the following diagram is the visual representation of the proving workflow (RISC0 continuations).

![Bento diagram](/bento-diagram.png)

From there Bento has the application containers

- REST API
- Agents (of different work types exec/gpu/aux/snark)

As demonstrated above, Bento breaks down tasks into these major actions:

- Init/Setup (executor) - This action generates continuations or "segments" to be proven and places them on Redis.
- Prove + lift (GPU/CPU agent) - Proves a segment on a CPU or GPU and lifts the result to Redis.
- Join - takes two lifted proofs and joins them together into one proof.
- Resolve - produce a final join - Resolve verifies all the unverified claims, effectively completing any composition tasks.
- Finalize - Uploads the final proof to minio.
- SNARK - Convert a STARK proof into a SNARK proof using [rapidsnark](https://github.com/iden3/rapidsnark).

:::tip[Note]
For a more in depth information see [the recursive proving docs][r0-docs-recursion].
:::

### Redis

In order to share intermediate files (such as Segments) between workers, Redis is using as fast intermediary. Bento writes to Redis for fast cross machine file access and provides a high bandwidth backbone for sharing data between nodes and workers.

:::warning[Warning]
The Redis node's memory configuration is important for the size of proofs running. Because each segment is \~5 - 10 MB in size it is possible to overload Redis's node memory with too much data if the STARK proof is large enough and the GPU workers are not consuming the segments fast enough.

We recommend a high memory node for the Redis container as well as active monitoring / alerts (See Grafana for monitor) on the Redis node to ensure it does not overflow the possible memory.
:::

### TaskDB

TaskDB is the center of how Bento schedules and prioritizes work. It provides us the ability to create a job which will contain many tasks, each with different actions in a stream of work. This stream is ordered by priority and dependencies. TaskDB's core job is to correctly emit work to agents via long polling in the right order and priority. As segments stream out of the executor TaskDB delegates the work plan such that GPU nodes can start proving before the executor completes.

TaskDB also has the ability to prioritize different work over others using two separate modes:

- Priority multiplier
- Dedicated resources

#### The Priority Multiplier

Priority multiplier allows for individual users and task types to be schedules ahead of other users.

#### The Dedicated Resources Mode

Dedicated Resources allows for a stream's user to get priority access to N workers on that stream. For example if user1 has 10 GPU stream dedicated resources then that work will always get priority over the normal pool of users that have dedicated count of 0. But once user1 has 10 concurrent GPU tasks, any additional work is scheduled with the rest of the priority pool of user work.

### The Agent

Bento agents are long polling daemons that opt in to specific actions. An agent can be configured to act as a:

- Executor
- GPU worker
- CPU worker
- SNARK agent.

This allows Bento to run on diverse hardware within a cluster that can specialize in tasks that need specific hardware.

Some examples of key hardware requirements for each stream:

- Executor - Needs low core count but very high single thread core clock CPU performance
- GPU - Needs a GPU device to run the risc0 GPU accelerated proving
- CPU (optional) - run prove+lift on a CPU instead of a GPU, not advised
- SNARK - Needs a high CPU thread count and core speed node.

The agent polls for work, runs the work, monitors for failures and reports status back to TaskDB.

### More on Executor

The executor (init) task is the first process run within a STARK proving workflow and iteratively generates the continuations work plan of prove+lift, join, resolve and finalize.

Internally each "user" of Bento gets their own stream for each type of work. So user1 would have their own stream for CPU, GPU, Aux, and SNARK work types.

Then each stream has settings for priority multiplier and dedicated resources described above.

### More on the GPU

The GPU agent does the heavy lifting of proving itself. Work is broken into powers of 2 segments sizes (128K, 256K, 500K, 1M, 2M, 4M cycles). Which GPU you have will dictate which power of 2 you will select. As a general rule of thumb:

- 1mcycle (1 million cycles) requires 9\~10GB of GPU VRAM
- 2mcycle (2 million cycles) requires 17\~18GB of GPU VRAM
- 4mcycle (4 million cycles) requires 32\~34GB of GPU VRAM

### More on SNARK

This agent will convert a STARK proof into a SNARK proof using [rapidsnark](https://github.com/iden3/rapidsnark). Performance is dependent on core clocks AND thread counts. Lots of cores but a very low core clock can adversely affect performance.

### REST API

The REST API provides a external interface to start / stop / monitor jobs and tasks within TaskDB.

:::warning[Warning]
TODO: Write more here about brokers REST API
:::

[page-broker]: /prover-manual/broker/introduction
[r0-docs-recursion]: https://dev.risczero.com/api/recursion
