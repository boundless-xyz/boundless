# Grafana Role

This role installs and configures Grafana for monitoring and visualization of the Boundless prover stack.

## Requirements

- Ansible 2.9 or higher
- Debian/Ubuntu-based system
- Root or sudo access

## Role Variables

### Installation

- `grafana_install` (default: `true`): Whether to install Grafana
- `grafana_version` (default: `"11.0.0"`): Grafana version to install (use `"latest"` for latest release)

### Service Configuration

- `grafana_service_enabled` (default: `true`): Enable Grafana service at boot
- `grafana_service_state` (default: `started`): Service state (`started`, `stopped`, `restarted`)

### Network Configuration

- `grafana_host` (default: `"localhost"`): Grafana domain/hostname
- `grafana_port` (default: `3000`): HTTP port for Grafana
- `grafana_listen_address` (default: `"0.0.0.0"`): Address to bind to

### Security

- `grafana_admin_user` (default: `"admin"`): Admin username (can be set via `GRAFANA_ADMIN_USER` env var)
- `grafana_admin_password` (default: `"admin"`): Admin password (can be set via `GRAFANA_ADMIN_PASSWORD` env var)
- `grafana_log_level` (default: `"WARN"`): Log level (can be set via `GRAFANA_LOG_LEVEL` env var)

### Plugins

- `grafana_plugins` (default: `["frser-sqlite-datasource"]`): List of plugins to install

### Directories

- `grafana_data_dir` (default: `"/var/lib/grafana"`): Data directory
- `grafana_config_dir` (default: `"/etc/grafana"`): Configuration directory
- `grafana_log_dir` (default: `"/var/log/grafana"`): Log directory
- `grafana_provisioning_dir` (default: `"/etc/grafana/provisioning"`): Provisioning directory

### PostgreSQL Datasource

- `grafana_postgresql_enabled` (default: `true`): Enable PostgreSQL datasource
- `grafana_postgresql_host` (default: `postgresql_host`): PostgreSQL host
- `grafana_postgresql_port` (default: `postgresql_port`): PostgreSQL port
- `grafana_postgresql_database` (default: `postgresql_database`): PostgreSQL database name
- `grafana_postgresql_user` (default: `postgresql_user`): PostgreSQL username
- `grafana_postgresql_password` (default: `postgresql_password`): PostgreSQL password

### Broker SQLite Datasource

- `grafana_broker_sqlite_enabled` (default: `true`): Enable Broker SQLite datasource
- `grafana_broker_db_path` (default: `"/etc/boundless/broker.db"`): Path to broker SQLite database

### Dashboard Provisioning

- `grafana_dashboards_enabled` (default: `true`): Enable dashboard provisioning
- `grafana_dashboards_dir` (default: `"/etc/grafana/provisioning/dashboards"`): Dashboard directory
- `grafana_dashboard_files` (default: `[]`): List of dashboard JSON files to copy (paths relative to playbook)

## Example Playbook

```yaml
- name: Deploy Grafana
  hosts: all
  become: true
  gather_facts: true

  vars:
    grafana_admin_password: "secure_password_here"
    grafana_dashboard_files:
      - "{{ playbook_dir }}/../bento-grafana-dashboard.json"

  tasks:
    - name: Deploy Grafana
      ansible.builtin.include_role:
        name: grafana
```

## Tags

- `grafana`: All Grafana tasks
- `grafana-config`: Configuration tasks only
- `grafana-dashboards`: Dashboard provisioning tasks only
- `grafana-plugins`: Plugin installation tasks only

## Dependencies

None

## License

Apache-2.0
