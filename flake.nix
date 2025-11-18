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
        python = pkgs.python3.withPackages (ps: with ps; [
          urllib3
        ]);
      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = [
            python
            pkgs.nodejs_22
          ];
        };
      }
    );
}
