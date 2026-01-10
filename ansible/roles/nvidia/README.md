# NVIDIA Role

This role installs NVIDIA drivers and CUDA Toolkit.

## Requirements

- Ubuntu 22.04 or 24.04
- Root/sudo access
- Internet connectivity to download NVIDIA packages
- GPU hardware (for verification)

## Role Variables

- `nvidia_cuda_version` (default: `"13-0"`): CUDA version to install (format: "13-0" for CUDA 13.0)
- `nvidia_ubuntu_version` (default: `null`): Ubuntu version override (auto-detected if null)
- `nvidia_reboot_after_install` (default: `false`): Automatically reboot after installation

## Dependencies

None.

## Example Playbook

```yaml
---
- hosts: provers
  become: true
  roles:
    - role: nvidia
      vars:
        nvidia_cuda_version: "13-0"
        nvidia_reboot_after_install: false  # Manual reboot recommended
```

## What This Role Does

1. **Detects Ubuntu version** and maps it to the appropriate CUDA repository
2. **Installs NVIDIA CUDA keyring** for package repository access
3. **Installs CUDA Toolkit** (`cuda-toolkit-13-0`) and NVIDIA open-source drivers (`nvidia-open`)
4. **Verifies installation** using `nvidia-smi`
5. **Optionally reboots** the system if drivers need to be loaded

## Important Notes

- **Reboot Required**: NVIDIA drivers require a system reboot to load. The role can automatically reboot, but manual reboot is recommended for production.
- **GPU Detection**: After reboot, run `nvidia-smi` to verify GPU detection.

## Tags

- `nvidia` - All NVIDIA-related tasks
- `nvidia-install` - CUDA Toolkit installation
- `nvidia-verify` - Verification tasks

## License

BSD/MIT

## Author Information

Boundless
