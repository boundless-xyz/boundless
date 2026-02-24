# UFW Role

This Ansible role installs and configures UFW (Uncomplicated Firewall) on Ubuntu systems.

## Features

- Install UFW
- Configure default policies (deny incoming, allow outgoing)
- Allow specific ports
- Rate limit ports (e.g., SSH brute-force protection)
- Allow specific networks/IPs
- Configure logging level

## Requirements

- Ubuntu 22.04 or 24.04

## Role Variables

### Basic Configuration

| Variable               | Default | Description               |
| ---------------------- | ------- | ------------------------- |
| `ufw_enabled`          | `true`  | Enable/disable UFW        |
| `ufw_default_incoming` | `deny`  | Default incoming policy   |
| `ufw_default_outgoing` | `allow` | Default outgoing policy   |
| `ufw_ssh_port`         | `22`    | SSH port (always allowed) |
| `ufw_logging`          | `low`   | Logging level             |

### Port Configuration

| Variable                 | Default                    | Description              |
| ------------------------ | -------------------------- | ------------------------ |
| `ufw_allowed_ports`      | `[{port: 22, proto: tcp}]` | Ports to allow           |
| `ufw_rate_limited_ports` | `[]`                       | Ports with rate limiting |
| `ufw_allowed_networks`   | `[]`                       | Networks/IPs to allow    |

### Docker NAT (when Docker daemon has `"iptables": false`)

When the Docker daemon has `"iptables": false` (e.g. to avoid conflicts with UFW), Docker does not add NAT/MASQUERADE rules, so containers cannot reach the internet. This role can add the required NAT and FORWARD rules to `/etc/ufw/before.rules`:

| Variable                 | Default         | Description                                   |
| ------------------------ | --------------- | --------------------------------------------- |
| `ufw_docker_nat_enabled` | `true`          | Add NAT and FORWARD for Docker default bridge |
| `ufw_docker_subnet`      | `172.17.0.0/16` | Docker default bridge subnet                  |

Set `ufw_docker_nat_enabled: true` for hosts that run Docker with `iptables: false` and need container internet access.

## Usage

### Basic (SSH only)

```yaml
- name: Configure firewall
  ansible.builtin.include_role:
    name: ufw
```

### Web Server

```yaml
- name: Configure firewall
  ansible.builtin.include_role:
    name: ufw
  vars:
    ufw_allowed_ports:
      - { port: 22, proto: tcp, comment: "SSH" }
      - { port: 80, proto: tcp, comment: "HTTP" }
      - { port: 443, proto: tcp, comment: "HTTPS" }
```

### Prover Node

```yaml
- name: Configure firewall
  ansible.builtin.include_role:
    name: ufw
  vars:
    ufw_allowed_ports:
      - { port: 22, proto: tcp, comment: "SSH" }
      - { port: 80, proto: tcp, comment: "HTTP (nginx)" }
      - { port: 8081, proto: tcp, comment: "REST API" }
    ufw_rate_limited_ports:
      - { port: 22, proto: tcp, comment: "SSH rate limit" }
```

### With Network Restrictions

```yaml
- name: Configure firewall
  ansible.builtin.include_role:
    name: ufw
  vars:
    ufw_allowed_ports:
      - { port: 22, proto: tcp, comment: "SSH" }
    ufw_allowed_networks:
      - { from: "10.0.0.0/8", port: 5432, proto: tcp, comment: "PostgreSQL internal" }
      - { from: "192.168.1.0/24", comment: "Office network" }
```

## Inventory Configuration

```yaml
all:
  hosts:
    prover-1:
      ufw_allowed_ports:
        - { port: 22, proto: tcp, comment: "SSH" }
        - { port: 80, proto: tcp, comment: "HTTP" }
```

## Service Management

```bash
# Check status
sudo ufw status verbose

# Check numbered rules
sudo ufw status numbered

# Delete a rule by number
sudo ufw delete 3

# Reload rules
sudo ufw reload

# View logs
sudo tail -f /var/log/ufw.log
```

## Tags

- `ufw` - All UFW tasks
- `ufw-install` - Installation only
- `ufw-config` - Configuration only
- `ufw-rules` - Firewall rules only
- `ufw-enable` - Enable UFW
- # `ufw-disable` - Disable UFW
  The role tasks currently do not define dedicated Ansible tags.

## Dependencies

None
