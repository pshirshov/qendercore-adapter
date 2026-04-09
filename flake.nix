{
  description = "Qendercore adapter";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        rustAdapter = pkgs.rustPlatform.buildRustPackage {
          pname = "qendercore-mqtt-adapter";
          version = "0.1.0";
          src = ./rust-mqtt-adapter;
          cargoLock = {
            lockFile = ./rust-mqtt-adapter/Cargo.lock;
          };
        };
      in
      {
        packages = {
          qendercore-mqtt-adapter = rustAdapter;
          default = rustAdapter;
        };
        devShells.default = pkgs.mkShell {
          buildInputs = [
            pkgs.nodejs_22
            pkgs.cargo
            pkgs.rustc
            pkgs.rustfmt
            pkgs.clippy
            pkgs.rust-analyzer
          ];
        };
      }
    );
}
