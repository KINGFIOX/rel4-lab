{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-overlay,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          config.allowUnfree = true;
          overlays = [
            rust-overlay.overlays.default
          ];
        };

        riscvToolchain = [
          pkgs.pkgsCross.riscv64-embedded.buildPackages.gcc
        ];

        rustToolchain = [
          (pkgs.rust-bin.stable.latest.default.override {
            extensions = [
              "rust-src"
              "rust-analyzer"
            ];
          })
        ];

      in
      {
        devShells.default = pkgs.mkShell {

          packages =
            pkgs.lib.concatLists [
              riscvToolchain
              rustToolchain
            ]
            ++ (with pkgs; [
              qemu
            ]);

          shellHook = "";
        };
      }
    );
}
