# Valkey Role

This role installs and configures Valkey (Redis fork) server.

## Requirements

* Ansible 2.9 or later
* Ubuntu 22.04 or 24.04
* System with systemd
* Sudo/root privileges

## Role Variables

### Optional Variables

* `valkey_install` (default: `true`): Install Valkey
* `valkey_service_enabled` (default: `true`): Enable service to start on boot
* `valkey_service_state` (default: `started`): Service state
* `valkey_host` (default: `"localhost"`): Valkey host
* `valkey_port` (default: `6379`): Valkey port
* `valkey_protected_mode` (default: `false`): Enable protected mode
* `valkey_maxmemory` (default: `"12gb"`): Maximum memory
* `valkey_maxmemory_policy` (default: `"allkeys-lru"`): Memory eviction policy

## Dependencies

None

## Example Playbook

```yaml
---
- hosts: cache
  become: true
  roles:
    - role: valkey
      vars:
        valkey_maxmemory: "8gb"
        valkey_host: "127.0.0.1"
        valkey_protected_mode: true
```

## License

BSD/MIT

## Author Information

Boundless
