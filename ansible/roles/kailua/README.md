# Kailua role

Installs [Kailua](https://github.com/boundless-xyz/kailua) and optionally runs it as a systemd service.

**Default: Docker** – Builds the image from the [official Dockerfile](https://github.com/boundless-xyz/kailua/blob/main/Dockerfile) (no Rust/RISC0/Foundry on the host). The service runs `docker run ... kailua-cli <args>`.

**Optional: Source** – Builds on the host (requires rust role and `kailua_install_deps`).

## Requirements

* Target host: Linux (Ubuntu/Debian assumed for apt). Docker method requires Docker (docker role is a dependency).
* Network access to GitHub; for source method also risczero.com, foundry.paradigm.xyz; for optional SSM/CloudWatch, AWS endpoints.

## Install methods

| Method   | Default | Description |
|----------|---------|-------------|
| `docker` | **yes** | Clone repo at `kailua_version`, `docker build` from Dockerfile, run via `docker run ... kailua-cli`. Image is built only when missing (idempotent). |
| `source` | no      | Install deps (apt, RISC0, svm, Foundry; Rust from rust role), download source, `cargo build`, install binary to `kailua_install_dir`. |

## Role variables

| Variable | Default | Description |
|----------|--------|-------------|
| `kailua_install_method` | `docker` | `docker` or `source`. |
| `kailua_version` | `v1.1.9` | GitHub release tag (for Docker build and source download). |
| `kailua_docker_image` | `kailua:{{ kailua_version }}` | Docker image name to build/use. |
| `kailua_docker_build_dir` | `/tmp/kailua-build` | Clone repo here to build image. |
| `kailua_repo_url` | `https://github.com/boundless-xyz/kailua.git` | Repo URL for Docker build. |
| `kailua_docker_clean_build_dir` | `true` | Remove clone dir after building image. |
| `kailua_docker_network_mode` | `""` | Docker run network (e.g. `host` for host networking). |
| `kailua_download_url` | source archive URL | Source method: release tarball URL. |
| `kailua_install_dir` | `/usr/local/bin` | Source method: binary install dir. |
| `kailua_install_deps` | `true` | Source method: install apt/RISC0/svm/Foundry (Rust from rust role). |
| `kailua_build_home`, `kailua_rust_cargo_bin`, `kailua_svm_version` | (see defaults) | Source method only. |
| `kailua_install_aws_ssm_cloudwatch` | `false` | Optional SSM + CloudWatch (AWS CLI from another role). |
| `kailua_install_service` | `true` | Install and enable systemd unit. |
| `kailua_service_args` | `""` | Extra args (e.g. `devnet`). For Docker: passed to `kailua-cli`. |

## Example

```yaml
- hosts: kailua_hosts
  roles:
    - role: kailua
      vars:
        kailua_version: v1.1.9
        # Default: docker (build image, run kailua-cli in container)
        kailua_install_method: docker
        kailua_service_args: devnet
        # Optional: host networking for the container
        # kailua_docker_network_mode: host
```

## Notes

* **Docker**: Depends on docker role. Builds image only when `docker image inspect <kailua_docker_image>` fails (idempotent). Service runs `docker run --rm --name kailua <image> kailua-cli <kailua_service_args>`.
* **Source**: Uses the rust role (included when `kailua_install_method == "source"`); use `kailua_install_deps: true` to install RISC0/svm/Foundry. Skips build if `kailua --version` succeeds. The rust role must be available in your roles path.
* AWS CLI is expected from another role (e.g. `awscli`). SSM/CloudWatch are optional via `kailua_install_aws_ssm_cloudwatch`.
