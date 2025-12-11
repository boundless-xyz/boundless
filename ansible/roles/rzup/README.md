# RZUP Role

This Ansible role installs RISC Zero's `rzup` toolchain manager and optionally the `risc0-groth16` and `blake3-groth16` components, which are required for Groth16 proof generation.

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
* `rzup_install_blake3_groth16` (default: `true`): Whether to install blake3-groth16 component
* `rzup_risc0_home` (default: `null`): RISC0\_HOME directory (defaults to `$HOME/.risc0`)

## Installation

The role:

1. Installs `rzup` using the RISC Zero install script to `$HOME/.risc0/`
2. Creates a symlink in `/usr/local/bin/rzup` for system-wide access
3. Optionally installs the `risc0-groth16` component if `rzup_install_groth16` is true
4. Optionally installs the `blake3-groth16` component if `rzup_install_blake3_groth16` is true

**Important**: When installing both `risc0-groth16` and `blake3-groth16`, they are installed in sequence to the same RISC0\_HOME directory. The installation order is:

* `risc0-groth16` is installed first
* `blake3-groth16` is installed second

Both components use separate binary names (`risc0-groth16` vs `blake3-groth16`) and should coexist without conflicts. The role includes a final verification step to ensure both components are properly installed and can coexist.

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
        rzup_install_blake3_groth16: true
```

## Tags

* `rzup`: All rzup installation tasks
* `rzup-install`: rzup installation tasks
* `rzup-groth16`: risc0-groth16 installation tasks
* `rzup-blake3-groth16`: blake3-groth16 installation tasks

## Dependencies

* `rust` role (automatically installed as a dependency)
