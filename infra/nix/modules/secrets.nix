{ config, pkgs, ... }:

{
  # SOPS configuration
  sops = {
    # Age key file location
    age.keyFile = "/var/lib/sops-nix/key.txt";

    # Default secrets configuration
    defaultSopsFile = ./secrets/secrets.yaml;

    # Secrets definitions
    secrets = {
      # Database credentials
      "database_password" = {
        owner = "boundless";
        group = "boundless";
        mode = "0400";
      };

      # Ethereum private key
      "ethereum_private_key" = {
        owner = "boundless";
        group = "boundless";
        mode = "0400";
      };

      # MinIO credentials
      "minio_access_key" = {
        owner = "boundless";
        group = "boundless";
        mode = "0400";
      };

      "minio_secret_key" = {
        owner = "boundless";
        group = "boundless";
        mode = "0400";
      };

      # Redis password (if needed)
      "redis_password" = {
        owner = "boundless";
        group = "boundless";
        mode = "0400";
      };

      # JWT secrets for API authentication
      "jwt_secret" = {
        owner = "boundless";
        group = "boundless";
        mode = "0400";
      };

      # AWS credentials (if using AWS services)
      "aws_access_key_id" = {
        owner = "boundless";
        group = "boundless";
        mode = "0400";
      };

      "aws_secret_access_key" = {
        owner = "boundless";
        group = "boundless";
        mode = "0400";
      };
    };
  };

  # Install SOPS and age
  environment.systemPackages = with pkgs; [
    sops
    age
    age-keygen
  ];

  # Create sops-nix directory
  systemd.tmpfiles.rules = [
    "d /var/lib/sops-nix 0755 root root -"
  ];
}
