{
  description = "jimini-normalizer - Multi-Provider Data Normalization Service";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    substrate = {
      url = "github:pleme-io/substrate";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.fenix.follows = "fenix";
    };
    forge = {
      url = "github:pleme-io/forge";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.fenix.follows = "fenix";
      inputs.substrate.follows = "substrate";
      inputs.crate2nix.follows = "crate2nix";
    };
    crate2nix = {
      url = "github:nix-community/crate2nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, substrate, forge, crate2nix, ... }:
    let
      # Base service flake from substrate (provides Rust toolchain, devShell, Docker image, etc.)
      base = (import "${substrate}/lib/rust-service-flake.nix" {
        inherit nixpkgs substrate forge crate2nix;
      }) {
        inherit self;
        serviceName = "jimini-normalizer";
        registry = "ghcr.io/pleme-io/jimini-normalizer";
        packageName = "jimini-normalizer";
        namespace = "jimini-normalizer-system";
        architectures = ["amd64" "arm64"];
        moduleDir = null;
        nixosModuleFile = null;
        serviceType = "rest";
        ports = { http = 8080; health = 8080; metrics = 9090; };
      };

      # Extend devShell with additional tools for this service
      forEachSystem = nixpkgs.lib.genAttrs [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
    in
    base // {
      apps = forEachSystem (system:
        let
          pkgs = import nixpkgs { inherit system; };
          configAdmin = base.packages.${system}."config-admin" or base.packages.${system}.default;
          mkConfigApp = subcommand: {
            type = "app";
            program = toString (pkgs.writeShellScript "config-${subcommand}" ''
              exec ${configAdmin}/bin/config-admin ${subcommand} "$@"
            '');
          };
        in {
          config-list = mkConfigApp "list";
          config-get = mkConfigApp "get";
          config-set = mkConfigApp "set";
          config-delete = mkConfigApp "delete";
        } // (base.apps.${system} or {})
      );

      devShells = forEachSystem (system:
        let
          pkgs = import nixpkgs { inherit system; };
          baseShell = base.devShells.${system}.default or (pkgs.mkShell {});
        in {
          default = pkgs.mkShell {
            inputsFrom = [ baseShell ];
            buildInputs = with pkgs; [
              # Database tooling
              sea-orm-cli

              # NATS tooling
              natscli

              # Grafana dashboards as code
              jsonnet
              go-jsonnet

              # Container tooling
              docker-compose

              # HTTP testing
              curl
              jq

              # Helm chart development
              kubernetes-helm
            ];

            shellHook = ''
              # Default env vars for local development
              export DATABASE_URL="postgres://jimini:jimini@localhost:5432/jimini"
              export NATS_URL="nats://localhost:4222"
              export RUST_LOG="info,jimini_normalizer=debug"
              export RUN_MODE="api"
            '';
          };
        }
      );
    };
}
