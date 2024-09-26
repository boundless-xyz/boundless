# Broker Node

The broker node is a optional addon service that runs within the Bento docker-compose stack. Before running broker you will need to ensure you
have setup and are able to run Bento, the documentation for that can be found in [Running Bento](../bento/running_bento.md)

Optionally if you can't run a Bento stack is is possible to run Broker using the Bonsai proving backend adding the `--bonsai-api-url` and `--bonsai-api-key` flags in the Service CLI config.

# Running

```bash
docker compose --profile broker --env-file ./.env-compose up --build
```

# Configuration

There are two layers of configuration for Broker:

- Service daemon config - Setup / wallet keys / IPs
- Live configuration - Market parameters / Prover configs / Batching / aggregator configs

## Service config

The service can be configured via CLI flags which can be supplied in the `compose.yml` broker->entrypoint. These are mostly for configuration of the service itself and private key material. Many of the vars can be found in `.env-compose` which are passed through to the services

## Live configuration

The docker-compose project bind mounts the `broker.toml` in the root of the project into the container, here you can configure all the different parameters of the market side of the daemon. Additionally this file is dynamically update the config if it is changed on the fly. Most parameters will automatically reload and re-apply async and can be used to adjust broker configurations on the fly without restarting the daemon.
