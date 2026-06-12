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

        crossToolchains = [
          pkgs.pkgsCross.riscv64-embedded.buildPackages.gcc
          pkgs.pkgsCross.loongarch64-linux-embedded.buildPackages.gcc
        ];

        rustToolchain = [
          (pkgs.rust-bin.stable.latest.default.override {
            extensions = [
              "rust-src"
              "rust-analyzer"
            ];
            targets = [
              "riscv64gc-unknown-none-elf"
              "loongarch64-unknown-none"
            ];
          })
        ];

        pythonEnv = pkgs.python3.withPackages (
          ps:
          with ps; [
            lxml
            jinja2
            jsonschema
            ply
            pyelftools
            pyfdt
            protobuf
            pyyaml
          ]
        );

      in
      {
        devShells.default = pkgs.mkShell {

          packages =
            pkgs.lib.concatLists [
              crossToolchains
              rustToolchain
            ]
            ++ (with pkgs; [
              cmake
              qemu
              ninja
              dtc
              protobuf
              cpio
              pythonEnv
            ]);

          shellHook = ''
            # Avoid macOS host's BSD ar/ranlib leaking into the seL4 elfloader
            # rebuild step that we drive via ninja in tools/pack-image.py.
            unset AR AS CC CXX LD NM OBJCOPY OBJDUMP RANLIB READELF SIZE STRINGS STRIP
            unset HOST_CC HOST_CXX BUILD_CC
            unset CFLAGS CXXFLAGS LDFLAGS CPPFLAGS

            # OpenSBI's Makefile uses `greadlink` (Homebrew naming). Nix's
            # coreutils ships `readlink`, so route the var through.
            export READLINK=readlink
          '';
        };
      }
    );
}
