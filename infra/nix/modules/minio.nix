{ config, pkgs, ... }:

{
  # MinIO-specific configuration
  networking.hostName = "boundless-minio";

  # Enable MinIO
  services.minio = {
    enable = true;
    dataDir = "/var/lib/minio";
    listenAddress = ":9000";
    consoleAddress = ":9001";
    rootCredentialsFile = "/etc/minio/credentials";
  };

  # MinIO credentials
  environment.etc."minio/credentials" = {
    text = ''
      MINIO_ROOT_USER=minioadmin
      MINIO_ROOT_PASSWORD=minioadmin123
    '';
    mode = "0600";
  };

  # Create MinIO bucket
  systemd.services.minio-bucket-setup = {
    wantedBy = [ "multi-user.target" ];
    after = [ "minio.service" ];
    serviceConfig = {
      Type = "oneshot";
      ExecStart = pkgs.writeShellScript "minio-bucket-setup" ''
        # Wait for MinIO to be ready
        until curl -f http://localhost:9000/minio/health/live; do
          echo "Waiting for MinIO to be ready..."
          sleep 5
        done

        # Configure MinIO client
        ${pkgs.minio-client}/bin/mc alias set local http://localhost:9000 minioadmin minioadmin123

        # Create bucket
        ${pkgs.minio-client}/bin/mc mb local/boundless-bento-data --ignore-existing

        # Set bucket policy to public read
        ${pkgs.minio-client}/bin/mc anonymous set public local/boundless-bento-data

        echo "MinIO bucket setup complete"
      '';
    };
  };

  # Firewall rules
  networking.firewall.allowedTCPPorts = [ 9000 9001 ];

  # Monitoring
  systemd.services.minio-monitor = {
    wantedBy = [ "multi-user.target" ];
    after = [ "minio.service" ];
    serviceConfig = {
      Type = "simple";
      ExecStart = pkgs.writeShellScript "minio-monitor" ''
        while true; do
          curl -f http://localhost:9000/minio/health/live > /dev/null 2>&1
          if [ $? -eq 0 ]; then
            echo "MinIO is healthy"
          else
            echo "MinIO is unhealthy"
          fi
          sleep 60
        done
      '';
    };
  };

  # Health check endpoint
  systemd.services.minio-health = {
    wantedBy = [ "multi-user.target" ];
    after = [ "minio.service" ];
    serviceConfig = {
      Type = "oneshot";
      ExecStart = pkgs.writeShellScript "minio-health" ''
        curl -f http://localhost:9000/minio/health/live || exit 1
        echo "MinIO health check passed"
      '';
    };
  };
}
