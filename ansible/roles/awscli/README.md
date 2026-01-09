# AWS CLI Role

This role installs and verifies AWS CLI v2 using the official AWS installer.

## Requirements

- Ubuntu (tested on 22.04 and 24.04)
- Root/sudo access
- Internet connectivity to download AWS CLI installer
- `unzip` package (automatically installed if not present)

## Role Variables

### Installation Method

- `awscli_install_method` (default: `"installer"`): Installation method
  - `"installer"`: Use official AWS CLI installer (recommended, latest version)
  - `"apt"`: Use apt package manager (may not be latest version)

### Installation Paths (installer method only)

- `awscli_install_dir` (default: `"/usr/local/aws-cli"`): Directory where AWS CLI is installed
- `awscli_bin_dir` (default: `"/usr/local/bin"`): Directory where `aws` symlink is created

### Installation Options

- `awscli_installer_url` (default: `"https://awscli.amazonaws.com/awscli-exe-linux-x86_64.zip"`): URL to AWS CLI installer
- `awscli_update` (default: `false`): Update existing installation if AWS CLI is already installed

## Dependencies

None.

## Example Playbook

```yaml
---
- hosts: all
  become: true
  roles:
    - role: awscli
      vars:
        awscli_install_method: "installer"
        awscli_update: false
```

## What This Role Does

1. **Checks if AWS CLI is already installed** and displays current version if found
2. **Installs unzip** (required for AWS CLI installer)
3. **Downloads AWS CLI installer** from official AWS URL
4. **Extracts the installer** to `/tmp/aws`
5. **Runs the installer** with specified installation and bin directories
6. **Verifies installation** by running `aws --version`
7. **Cleans up** installer files after installation

## Installation Methods

### Official Installer (Recommended)

The official installer method downloads and installs the latest AWS CLI v2 from AWS. This is the recommended method as it:

- Always installs the latest version
- Provides better version control
- Is officially supported by AWS

### Apt Package Manager (Alternative)

The apt method installs AWS CLI from the Ubuntu package repository. This method:

- May not have the latest version
- Is simpler but less flexible
- May require additional configuration

## Verification

The role automatically verifies the installation by running `aws --version` and will fail if the verification does not succeed.

## Tags

- `awscli` - All AWS CLI tasks
- `awscli-check` - Check if AWS CLI is installed
- `awscli-install` - Installation tasks
- `awscli-verify` - Verification tasks
- `awscli-cleanup` - Cleanup tasks

## References

- [AWS CLI Installation Documentation](https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html)

## License

BSD/MIT

## Author Information

Boundless
