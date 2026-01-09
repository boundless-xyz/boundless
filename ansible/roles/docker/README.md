# Docker Role

This role installs Docker Engine, Docker CLI, and Docker Compose plugin from the official Docker repository.

## Requirements

- Ubuntu 22.04 or 24.04
- Root/sudo access
- Internet connectivity to download Docker packages
- NVIDIA GPU hardware (optional, for GPU support)

## Role Variables

- `docker_ubuntu_version` (default: `null`): Ubuntu version override (auto-detected if null)
- `docker_architecture` (default: `"amd64"`): Architecture for Docker packages (amd64, arm64, etc.)
- `docker_users` (default: `[]`): List of users to add to the docker group (e.g., `["ubuntu", "deploy"]`)
- `docker_nvidia_enabled` (default: `false`): Enable NVIDIA Container Toolkit installation for GPU support

## Dependencies

None.

## Example Playbook

```yaml
---
- hosts: all
  become: true
  roles:
    - role: docker
      vars:
        docker_users:
          - ubuntu
          - deploy
        docker_nvidia_enabled: true  # Enable GPU support
```

## What This Role Does

1. **Installs prerequisites** (ca-certificates, curl, gnupg, lsb-release)
2. **Adds Docker's official GPG key** for package repository access
3. **Adds Docker repository** for the detected Ubuntu version
4. **Installs Docker packages**:
   - docker-ce (Docker Engine)
   - docker-ce-cli (Docker CLI)
   - containerd.io (Container runtime)
   - docker-buildx-plugin (Buildx plugin)
   - docker-compose-plugin (Docker Compose plugin)
5. **Starts and enables Docker service**
6. **Optionally adds users to docker group** (to run Docker without sudo)
7. **Verifies installation** using `docker --version`
8. **Optionally installs NVIDIA Container Toolkit** (if `docker_nvidia_enabled` is true and GPU is detected)

## Important Notes

- **Docker Compose**: The role installs Docker Compose as a plugin. Use `docker compose` (with a space) instead of `docker-compose` (with a hyphen).
- **User Permissions**: Users added to the docker group can run Docker commands without sudo. They may need to log out and back in for the group membership to take effect.
- **Architecture**: Defaults to amd64. Change `docker_architecture` for ARM systems.
- **NVIDIA GPU Support**: When `docker_nvidia_enabled` is true, the role will install and configure NVIDIA Container Toolkit. This requires NVIDIA drivers to be installed first (e.g., via the `nvidia` role).

## Tags

- `docker` - All Docker-related tasks
- `docker-install` - Docker package installation
- `docker-service` - Docker service management
- `docker-users` - User group management
- `docker-verify` - Verification tasks
- `docker-nvidia` - NVIDIA Container Toolkit installation and configuration

## License

BSD/MIT

## Author Information

Boundless
