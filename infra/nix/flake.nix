{
  description = "Boundless Infrastructure with NixOS";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-23.11";
    nixos-hardware.url = "github:NixOS/nixos-hardware";
    flake-utils.url = "github:numtide/flake-utils";
    sops-nix.url = "github:Mic92/sops-nix";
  };

  outputs = { self, nixpkgs, nixos-hardware, flake-utils, sops-nix, ... }@inputs:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
      in
      {
        packages = {
          manager = pkgs.callPackage ./packages/manager.nix { };
          prover = pkgs.callPackage ./packages/prover.nix { };
          execution = pkgs.callPackage ./packages/execution.nix { };
          aux = pkgs.callPackage ./packages/aux.nix { };
          broker = pkgs.callPackage ./packages/broker.nix { };
        };
      }) // {
    nixosConfigurations = {
      # Manager instance
      manager = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        modules = [
          sops-nix.nixosModules.sops
          ./modules/config.nix
          ./modules/manager.nix
          ./modules/common.nix
          ./modules/secrets.nix
          nixos-hardware.nixosModules.generic-x86-64
        ];
      };

      # Prover instances
      prover = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        modules = [
          sops-nix.nixosModules.sops
          ./modules/config.nix
          ./modules/prover.nix
          ./modules/common.nix
          ./modules/secrets.nix
          nixos-hardware.nixosModules.generic-x86-64
        ];
      };

      # Execution instances
      execution = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        modules = [
          sops-nix.nixosModules.sops
          ./modules/config.nix
          ./modules/execution.nix
          ./modules/common.nix
          ./modules/secrets.nix
          nixos-hardware.nixosModules.generic-x86-64
        ];
      };

      # Aux instances
      aux = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        modules = [
          sops-nix.nixosModules.sops
          ./modules/config.nix
          ./modules/aux.nix
          ./modules/common.nix
          ./modules/secrets.nix
          nixos-hardware.nixosModules.generic-x86-64
        ];
      };

      # Broker instances
      broker = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        modules = [
          sops-nix.nixosModules.sops
          ./modules/config.nix
          ./modules/broker.nix
          ./modules/common.nix
          ./modules/secrets.nix
          nixos-hardware.nixosModules.generic-x86-64
        ];
      };

      # PostgreSQL instances
      postgresql = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        modules = [
          sops-nix.nixosModules.sops
          ./modules/config.nix
          ./modules/postgresql.nix
          ./modules/common.nix
          ./modules/secrets.nix
          nixos-hardware.nixosModules.generic-x86-64
        ];
      };

      # MinIO instances
      minio = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        modules = [
          sops-nix.nixosModules.sops
          ./modules/config.nix
          ./modules/minio.nix
          ./modules/common.nix
          ./modules/secrets.nix
          nixos-hardware.nixosModules.generic-x86-64
        ];
      };
    };
  };
}
