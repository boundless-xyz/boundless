# Rust Role

This Ansible role installs the Rust programming language for a specified user.

## Requirements

* Ubuntu 22.04 or 24.04
* Sudo/root privileges

## Role Variables

### Installation Configuration

* `rust_install` (default: `true`): Whether to install Rust
* `rust_user` (default: `{{ ansible_user_id }}`): User to install Rust for
* `rust_group` (default: `{{ ansible_user_id }}`): Group for the user
* `rust_home` (default: `null`): Home directory for the user (auto-detected if null)
* `rust_install_method` (default: `"auto"`): Installation method
  * `"auto"`: Automatically detect based on Ubuntu version
  * `"rustup"`: Use rustup installer script (Ubuntu 22.04)
  * `"apt"`: Use apt package (Ubuntu 24.04)
* `rust_default_toolchain` (default: `"stable"`): Default Rust toolchain to install

## Installation Methods

### Ubuntu 22.04

Uses the rustup installer script (`curl https://sh.rustup.rs | sh`), which installs Rust to `$HOME/.cargo/`.

### Ubuntu 24.04

Uses the `rustup` apt package, then sets the default toolchain to stable.

## Example Playbook

```yaml
- hosts: all
  roles:
    - role: rust
      vars:
        rust_user: "bento"
        rust_home: "/var/lib/bento"
```

## Tags

* `rust`: All Rust installation tasks
* `rust-install`: Rust installation tasks
