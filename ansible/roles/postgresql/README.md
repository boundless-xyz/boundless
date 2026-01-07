# PostgreSQL Role

This role installs and configures PostgreSQL database server.

## Requirements

- Ansible 2.9 or later
- Ubuntu 22.04 or 24.04
- System with systemd
- Sudo/root privileges
- Ansible `community.postgresql` collection (must be installed separately)

## Role Variables

### Optional Variables

- `postgresql_install` (default: `true`): Install PostgreSQL
- `postgresql_service_enabled` (default: `true`): Enable service to start on boot
- `postgresql_service_state` (default: `started`): Service state
- `postgresql_user` (default: `"bento"`): Database user name
- `postgresql_password` (default: `"CHANGE_ME"`): Database user password
- `postgresql_host` (default: `"127.0.0.1"`): Database host
- `postgresql_port` (default: `5432`): Database port
- `postgresql_database` (default: `"bento"`): Database name
- `postgresql_create_db` (default: `true`): Create database
- `postgresql_create_user` (default: `true`): Create user

## Dependencies

**Important**: You must install the `community.postgresql` collection before using this role:

```bash
ansible-galaxy collection install community.postgresql
```

## Example Playbook

```yaml
---
- hosts: database
  become: true
  roles:
    - role: postgresql
      vars:
        postgresql_user: "myapp"
        postgresql_password: "secure_password"
        postgresql_database: "myapp_db"
```

## License

BSD/MIT

## Author Information

Boundless
