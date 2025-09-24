{ config, pkgs, ... }:

{
  # Execution-specific configuration
  networking.hostName = "boundless-execution";

  # Boundless Execution service
  systemd.services.boundless-execution = {
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {
      Type = "simple";
      User = "boundless";
      Group = "boundless";
      WorkingDirectory = "/opt/boundless";
      ExecStart = "${pkgs.boundless}/bin/agent -t exec --segment-po2 21";
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
        "BENTO_TASK_STREAM=exec"
        "BENTO_SEGMENT_PO2=21"
        "FINALIZE_RETRIES=3"
        "FINALIZE_TIMEOUT=10"
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

  # CPU monitoring
  systemd.services.cpu-monitor = {
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {
      Type = "simple";
      ExecStart = pkgs.writeShellScript "cpu-monitor" ''
        while true; do
          ${pkgs.util-linux}/bin/iostat -c 1 1 | tail -n +4 | awk '{print "CPU: " $1 " " $2 " " $3 " " $4}'
          sleep 60
        done
      '';
    };
  };
}
