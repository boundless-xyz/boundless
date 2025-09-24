{ config, pkgs, ... }:

{
  # Broker-specific configuration
  networking.hostName = "boundless-broker";

  # Boundless Broker service
  systemd.services.boundless-broker = {
    wantedBy = [ "multi-user.target" ];
    serviceConfig = {
      Type = "simple";
      User = "boundless";
      Group = "boundless";
      WorkingDirectory = "/opt/boundless";
      ExecStart = "${pkgs.boundless}/bin/broker --config-file /opt/boundless/broker.toml --bento-api-url http://10.0.1.100:8081 --db-url sqlite:///opt/boundless/broker.db";
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
    "d /opt/boundless 0755 boundless boundless -"
  ];

  # Firewall rules
  networking.firewall.allowedTCPPorts = [ 22 ];

  # Broker configuration file
  environment.etc."boundless/broker.toml" = {
    text = ''
      [database]
      url = "sqlite:///opt/boundless/broker.db"

      [redis]
      url = "redis://10.0.1.100:6379"

      [s3]
      bucket = "boundless-bento-data"
      url = "http://10.0.1.100:9000"
      region = "us-west-2"
      access_key = "minioadmin"
      secret_key = "minioadmin123"

      [ethereum]
      rpc_url = "https://your-rpc-url"
      private_key = "your-private-key"

      [bento_api]
      url = "http://10.0.1.100:8081"
    '';
    mode = "0644";
  };
}
