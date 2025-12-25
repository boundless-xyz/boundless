# RZUP Role

This Ansible role installs RISC Zero's `rzup` toolchain manager and optionally the `risc0-groth16` component, which is required for Groth16 proof generation.

**Note**: The `blake3-groth16` component is disabled by default (`rzup_install_blake3_groth16: false`) as it may not be a valid `rzup` component. If enabled, the installation will gracefully handle errors if the component is not available.

## Requirements

* Rust must be installed (this role depends on the `rust` role)
* Ubuntu 22.04 or 24.04
* Sudo/root privileges

## Role Variables

### Installation Configuration

* `rzup_install` (default: `true`): Whether to install rzup
* `rzup_user` (default: `{{ ansible_user_id }}`): User to install rzup for
* `rzup_group` (default: `{{ ansible_user_id }}`): Group for the user
* `rzup_home` (default: `null`): Home directory for the user (auto-detected if null)
* `rzup_install_groth16` (default: `true`): Whether to install risc0-groth16 component
* `rzup_install_blake3_groth16` (default: `false`): Whether to install blake3-groth16 component (**disabled by default** - may not be a valid component)
* `rzup_risc0_home` (default: `null`): RISC0\_HOME directory (defaults to `$HOME/.risc0`)

## Installation

The role:

1. Installs `rzup` using the RISC Zero install script to `$HOME/.risc0/`
2. Creates a symlink in `/usr/local/bin/rzup` for system-wide access
3. Optionally installs the `risc0-groth16` component if `rzup_install_groth16` is true
4. Optionally attempts to install the `blake3-groth16` component if `rzup_install_blake3_groth16` is true (disabled by default)

**Important Notes**:

* The `risc0-groth16` component is the primary component for Groth16 proof generation and is enabled by default
* The `blake3-groth16` component is disabled by default (`rzup_install_blake3_groth16: false`) as it may not be a valid `rzup` component
* If `blake3-groth16` installation is enabled but fails (e.g., "invalid value" error), the role will gracefully handle the error and continue without failing the entire playbook
* When both components are enabled, `risc0-groth16` is installed first, followed by `blake3-groth16` (if enabled)

## Example Playbook

```yaml
- hosts: all
  roles:
    - role: rust
      vars:
        rust_user: "bento"
        rust_home: "/var/lib/bento"
    - role: rzup
      vars:
        rzup_user: "bento"
        rzup_home: "/var/lib/bento"
        rzup_install_groth16: true
        rzup_install_blake3_groth16: false  # Disabled by default - may not be valid
```

## Tags

* `rzup`: All rzup installation tasks
* `rzup-install`: rzup installation tasks
* `rzup-groth16`: risc0-groth16 installation tasks
* `rzup-blake3-groth16`: blake3-groth16 installation tasks

## Dependencies

* `rust` role (automatically installed as a dependency)
