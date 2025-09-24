{ config, pkgs, ... }:

{
  # Manager-specific configuration
  networking.hostName = "boundless-manager";

  # Enable Docker
  virtualisation.docker = {
    enable = true;
    enableOnBoot = true;
    daemon.settings = {
      data-root = "/opt/boundless/docker";
    };
  };

  # PostgreSQL container
  virtualisation.oci-containers.containers.postgres = {
    image = "postgres:16";
    autoStart = true;
    ports = [ "5432:5432" ];
    environment = {
      POSTGRES_DB = "boundless_taskdb";
      POSTGRES_USER = "boundless";
    };
    environmentFiles = [
      config.sops.secrets."database_password".path
    ];
    volumes = [
      "/opt/boundless/data/postgres:/var/lib/postgresql/data"
    ];
  };

  # Redis container
  virtualisation.oci-containers.containers.redis = {
    image = "redis:7-alpine";
    autoStart = true;
    ports = [ "6379:6379" ];
    cmd = [ "redis-server" "--appendonly" "yes" ];
    volumes = [
      "/opt/boundless/data/redis:/data"
    ];
  };

  # MinIO container
  virtualisation.oci-containers.containers.minio = {
    image = "minio/minio";
    autoStart = true;
    ports = [ "9000:9000" "9001:9001" ];
    cmd = [ "server" "/data" "--console-address" ":9001" ];
    environmentFiles = [
      config.sops.secrets."minio_access_key".path
      config.sops.secrets."minio_secret_key".path
    ];
    volumes = [
      "/opt/boundless/data/minio:/data"
    ];
  };

  # Boundless API service
  systemd.services.boundless-api = {
    wantedBy = [ "multi-user.target" ];
    after = [ "docker.service" ];
    serviceConfig = {
      Type = "simple";
      User = "boundless";
      Group = "boundless";
      WorkingDirectory = "/opt/boundless";
      ExecStart = "${pkgs.boundless}/bin/api --bind-addr 0.0.0.0:8081 --snark-timeout 1800";
      Restart = "always";
      RestartSec = 10;
      Environment = [
        "RUST_LOG=info"
        "BENTO_API_LISTEN_ADDR=0.0.0.0"
        "BENTO_API_PORT=8081"
        "SNARK_TIMEOUT=1800"
        "DATABASE_URL=postgresql://boundless:${config.sops.secrets."database_password".path}@localhost:5432/boundless_taskdb"
        "REDIS_URL=redis://localhost:6379"
        "S3_BUCKET=boundless-bento-data"
        "S3_URL=http://localhost:9000"
        "AWS_REGION=us-west-2"
        "S3_ACCESS_KEY=${config.sops.secrets."minio_access_key".path}"
        "S3_SECRET_KEY=${config.sops.secrets."minio_secret_key".path}"
        "RPC_URL=https://your-rpc-url"
        "PRIVATE_KEY=${config.sops.secrets."ethereum_private_key".path}"
      ];
    };
  };

  # Boundless Broker service
  systemd.services.boundless-broker = {
    wantedBy = [ "multi-user.target" ];
    after = [ "docker.service" "boundless-api.service" ];
    serviceConfig = {
      Type = "simple";
      User = "boundless";
      Group = "boundless";
      WorkingDirectory = "/opt/boundless";
      ExecStart = "${pkgs.boundless}/bin/broker --config-file /opt/boundless/broker.toml --bento-api-url http://localhost:8081 --db-url sqlite:///opt/boundless/broker.db";
      Restart = "always";
      RestartSec = 10;
      Environment = [
        "RUST_LOG=info"
        "BENTO_BROKER_LISTEN_ADDR=0.0.0.0"
        "BENTO_BROKER_PORT=8082"
        "DATABASE_URL=postgresql://boundless:${config.sops.secrets."database_password".path}@localhost:5432/boundless_taskdb"
        "REDIS_URL=redis://localhost:6379"
        "S3_BUCKET=boundless-bento-data"
        "S3_URL=http://localhost:9000"
        "AWS_REGION=us-west-2"
        "S3_ACCESS_KEY=${config.sops.secrets."minio_access_key".path}"
        "S3_SECRET_KEY=${config.sops.secrets."minio_secret_key".path}"
        "RPC_URL=https://your-rpc-url"
        "PRIVATE_KEY=${config.sops.secrets."ethereum_private_key".path}"
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
    "d /opt/boundless/data/postgres 0755 boundless boundless -"
    "d /opt/boundless/data/redis 0755 boundless boundless -"
    "d /opt/boundless/data/minio 0755 boundless boundless -"
    "d /opt/boundless/docker 0755 boundless boundless -"
  ];

  # Firewall rules
  networking.firewall.allowedTCPPorts = [ 8081 8082 8083 8086 5432 6379 9000 9001 ];

  # Health checks
  systemd.services.boundless-health = {
    wantedBy = [ "multi-user.target" ];
    after = [ "boundless-api.service" "boundless-broker.service" ];
    serviceConfig = {
      Type = "oneshot";
      ExecStart = pkgs.writeShellScript "health-check" ''
        # Check PostgreSQL
        ${pkgs.postgresql}/bin/psql -h localhost -U boundless -d boundless_taskdb -c "SELECT 1;" || exit 1

        # Check Redis
        ${pkgs.redis}/bin/redis-cli -h localhost ping || exit 1

        # Check MinIO
        curl -f http://localhost:9000/minio/health/live || exit 1

        # Check API
        curl -f http://localhost:8081/health || exit 1

        echo "All services healthy"
      '';
    };
  };
}
