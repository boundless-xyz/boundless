# MinIO Role

This Ansible role installs and configures MinIO, an S3-compatible object storage server.

## Requirements

* Ansible 2.9 or higher
* Ubuntu 22.04 or 24.04
* Root or sudo access

## Role Variables

### Installation

* `minio_install` (default: `true`): Whether to install MinIO
* `minio_version` (default: `"latest"`): Version of MinIO to install (currently always uses latest)

### Service Configuration

* `minio_service_enabled` (default: `true`): Enable MinIO service at boot
* `minio_service_state` (default: `started`): Service state (started, stopped, restarted)

### User Configuration

* `minio_user` (default: `"minio"`): System user for MinIO service
* `minio_group` (default: `"minio"`): System group for MinIO service
* `minio_home` (default: `"/var/lib/minio"`): Home directory for MinIO user
* `minio_shell` (default: `"/usr/sbin/nologin"`): Shell for MinIO user

### Network Configuration

* `minio_host` (default: `"localhost"`): Hostname for MinIO
* `minio_port` (default: `9000`): Port for MinIO API
* `minio_console_port` (default: `9001`): Port for MinIO web console

### Credentials

* `minio_root_user` (default: `"minioadmin"`): Root user access key (should be changed)
* `minio_root_password` (default: `"minioadmin"`): Root user secret key (should be changed)

### Directory Configuration

* `minio_data_dir` (default: `"/var/lib/minio/data"`): Data directory for MinIO
* `minio_config_dir` (default: `"/etc/minio"`): Configuration directory
* `minio_log_dir` (default: `"/var/log/minio"`): Log directory
* `minio_install_dir` (default: `"/usr/local/bin"`): Installation directory for binary

### Lifecycle Configuration

* `minio_lifecycle_expire_days` (default: `2`): Number of days after which objects will be automatically expired (48 hours)
* `minio_lifecycle_bucket` (default: `"bento"`): Bucket name for lifecycle policy
  * Set `minio_lifecycle_expire_days` to `null` or `0` to disable lifecycle expiration

## Dependencies

None.

## Example Playbook

```yaml
- hosts: all
  become: true
  roles:
    - role: minio
      vars:
        minio_root_user: "my-access-key"
        minio_root_password: "my-secret-key"
        minio_port: 9000
```

## Integration with Bento Role

To use MinIO with the Bento role, set the following variables:

```yaml
- hosts: all
  become: true
  roles:
    - role: minio
      vars:
        minio_root_user: "bento-access-key"
        minio_root_password: "bento-secret-key"
        minio_port: 9000

    - role: bento
      vars:
        bento_s3_url: "http://localhost:9000"
        bento_s3_access_key: "{{ minio_root_user }}"
        bento_s3_secret_key: "{{ minio_root_password }}"
        bento_s3_bucket: "bento-storage"
```

## Notes

* The default credentials (`minioadmin`/`minioadmin`) should be changed in production
* MinIO will be accessible at `http://{{ minio_host }}:{{ minio_port }}` for API and `http://{{ minio_host }}:{{ minio_console_port }}` for web console
* The service runs as a dedicated `minio` user for security
* MinIO uses `MINIO_ROOT_USER` and `MINIO_ROOT_PASSWORD` environment variables for authentication

## License

Apache-2.0
