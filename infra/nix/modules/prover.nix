{ config, pkgs, ... }:

{
  # Prover-specific configuration
  networking.hostName = "boundless-prover";

  # NVIDIA drivers and CUDA
  services.xserver.videoDrivers = [ "nvidia" ];
  hardware.nvidia = {
    modesetting.enable = true;
    powerManagement.enable = true;
    open = false;
    nvidiaSettings = true;
  };

  # CUDA toolkit
  environment.systemPackages = with pkgs; [
    cudaPackages.cuda_cccl
    cudaPackages.cuda_cudart
    cudaPackages.cuda_cuobjdump
    cudaPackages.cuda_cupti
    cudaPackages.cuda_cuxxfilt
    cudaPackages.cuda_gdb
    cudaPackages.cuda_nvcc
    cudaPackages.cuda_nvdisasm
    cudaPackages.cuda_nvml_dev
    cudaPackages.cuda_nvprune
    cudaPackages.cuda_sanitizer_api
    cudaPackages.libcublas
    cudaPackages.libcufft
    cudaPackages.libcurand
    cudaPackages.libcusolver
    cudaPackages.libcusparse
    cudaPackages.libnpp
    cudaPackages.libnvjitlink
  ];

  # Boundless Prover service
  systemd.services.boundless-prover = {
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {
      Type = "simple";
      User = "boundless";
      Group = "boundless";
      WorkingDirectory = "/opt/boundless";
      ExecStart = "${pkgs.boundless}/bin/agent -t prove";
      Restart = "always";
      RestartSec = 10;
      Environment = [
        "RUST_LOG=info"
        "DATABASE_URL=postgresql://boundless:${config.sops.secrets."database_password".path}@10.0.1.100:5432/boundless_taskdb"
        "REDIS_URL=redis://10.0.1.100:6379"
        "S3_BUCKET=boundless-bento-data"
        "S3_URL=http://10.0.1.100:9000"
        "AWS_REGION=us-west-2"
        "S3_ACCESS_KEY=${config.sops.secrets."minio_access_key".path}"
        "S3_SECRET_KEY=${config.sops.secrets."minio_secret_key".path}"
        "PRIVATE_KEY=${config.sops.secrets."ethereum_private_key".path}"
        "BENTO_TASK_STREAM=prove"
      ];
    };
  };

  # Create boundless user
  users.users.boundless = {
    isSystemUser = true;
    group = "boundless";
    home = "/opt/boundless";
    createHome = true;
  };

  users.groups.boundless = {};

  # Create data directories
  systemd.tmpfiles.rules = [
    "d /opt/boundless 0755 boundless boundless -"
  ];

  # Firewall rules
  networking.firewall.allowedTCPPorts = [ 22 ];

  # GPU monitoring
  systemd.services.gpu-monitor = {
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {
      Type = "simple";
      ExecStart = pkgs.writeShellScript "gpu-monitor" ''
        while true; do
          ${pkgs.nvidia-settings}/bin/nvidia-smi --query-gpu=utilization.gpu,memory.used,memory.total --format=csv,noheader,nounits
          sleep 60
        done
      '';
    };
  };
}
