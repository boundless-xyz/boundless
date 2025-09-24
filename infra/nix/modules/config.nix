{ config, pkgs, ... }:

{
  # Configuration options for Boundless infrastructure
  options.boundless = {
    # Network configuration
    managerIp = pkgs.lib.mkOption {
      type = pkgs.lib.types.str;
      default = "10.0.1.100";
      description = "IP address of the manager instance";
    };

    # Database configuration
    database = {
      name = pkgs.lib.mkOption {
        type = pkgs.lib.types.str;
        default = "boundless_taskdb";
        description = "Database name";
      };
      user = pkgs.lib.mkOption {
        type = pkgs.lib.types.str;
        default = "boundless";
        description = "Database user";
      };
      port = pkgs.lib.mkOption {
        type = pkgs.lib.types.port;
        default = 5432;
        description = "Database port";
      };
    };

    # Redis configuration
    redis = {
      port = pkgs.lib.mkOption {
        type = pkgs.lib.types.port;
        default = 6379;
        description = "Redis port";
      };
    };

    # S3/MinIO configuration
    s3 = {
      bucket = pkgs.lib.mkOption {
        type = pkgs.lib.types.str;
        default = "boundless-bento-data";
        description = "S3 bucket name";
      };
      port = pkgs.lib.mkOption {
        type = pkgs.lib.types.port;
        default = 9000;
        description = "MinIO S3 port";
      };
      consolePort = pkgs.lib.mkOption {
        type = pkgs.lib.types.port;
        default = 9001;
        description = "MinIO console port";
      };
      region = pkgs.lib.mkOption {
        type = pkgs.lib.types.str;
        default = "us-west-2";
        description = "AWS region";
      };
    };

    # API configuration
    api = {
      port = pkgs.lib.mkOption {
        type = pkgs.lib.types.port;
        default = 8081;
        description = "API port";
      };
      listenAddr = pkgs.lib.mkOption {
        type = pkgs.lib.types.str;
        default = "0.0.0.0";
        description = "API listen address";
      };
    };

    # Broker configuration
    broker = {
      port = pkgs.lib.mkOption {
        type = pkgs.lib.types.port;
        default = 8082;
        description = "Broker port";
      };
      listenAddr = pkgs.lib.mkOption {
        type = pkgs.lib.types.str;
        default = "0.0.0.0";
        description = "Broker listen address";
      };
    };

    # Prover configuration
    prover = {
      port = pkgs.lib.mkOption {
        type = pkgs.lib.types.port;
        default = 8086;
        description = "Prover port";
      };
      listenAddr = pkgs.lib.mkOption {
        type = pkgs.lib.types.str;
        default = "0.0.0.0";
        description = "Prover listen address";
      };
    };

    # Execution configuration
    execution = {
      segmentPo2 = pkgs.lib.mkOption {
        type = pkgs.lib.types.int;
        default = 21;
        description = "Execution segment PO2";
      };
    };

    # Environment
    environment = pkgs.lib.mkOption {
      type = pkgs.lib.types.str;
      default = "development";
      description = "Environment name (dev, staging, prod)";
    };
  };

  # Default configuration values
  config = {
    # Set default values
    boundless = {
      # These can be overridden in specific modules or via NixOS configuration
    };
  };
}
