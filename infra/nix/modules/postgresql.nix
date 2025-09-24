{ config, pkgs, ... }:

{
  # PostgreSQL-specific configuration
  networking.hostName = "boundless-postgresql";

  # Enable PostgreSQL
  services.postgresql = {
    enable = true;
    package = pkgs.postgresql_16;
    dataDir = "/var/lib/postgresql/16";
    port = 5432;

    settings = {
      max_connections = 200;
      shared_buffers = "256MB";
      effective_cache_size = "1GB";
      maintenance_work_mem = "64MB";
      checkpoint_completion_target = 0.9;
      wal_buffers = "16MB";
      default_transaction_isolation = "read committed";
      random_page_cost = 1.1;
      effective_io_concurrency = 200;
      listen_addresses = "*";
    };

    authentication = ''
      local   all             all                                     md5
      host    all             all             127.0.0.1/32            md5
      host    all             all             10.0.0.0/8              md5
    '';

    ensureDatabases = [ "boundless_taskdb" ];
    ensureUsers = [
      {
        name = "boundless";
        ensureDBOwnership = true;
        ensureClauses = {
          password = "boundless_password";
        };
      }
    ];
  };

  # PostgreSQL backup
  services.postgresqlBackup = {
    enable = true;
    databases = [ "boundless_taskdb" ];
    location = "/var/backup/postgresql";
    startAt = "02:00";
  };

  # Firewall rules
  networking.firewall.allowedTCPPorts = [ 5432 ];

  # Monitoring
  systemd.services.postgresql-monitor = {
    wantedBy = [ "multi-user.target" ];
    after = [ "postgresql.service" ];
    serviceConfig = {
      Type = "simple";
      ExecStart = pkgs.writeShellScript "postgresql-monitor" ''
        while true; do
          ${pkgs.postgresql}/bin/psql -U boundless -d boundless_taskdb -c "SELECT version();" > /dev/null 2>&1
          if [ $? -eq 0 ]; then
            echo "PostgreSQL is healthy"
          else
            echo "PostgreSQL is unhealthy"
          fi
          sleep 60
        done
      '';
    };
  };

  # Health check endpoint
  systemd.services.postgresql-health = {
    wantedBy = [ "multi-user.target" ];
    after = [ "postgresql.service" ];
    serviceConfig = {
      Type = "oneshot";
      ExecStart = pkgs.writeShellScript "postgresql-health" ''
        ${pkgs.postgresql}/bin/psql -U boundless -d boundless_taskdb -c "SELECT 1;" || exit 1
        echo "PostgreSQL health check passed"
      '';
    };
  };
}
