{
  description = "Plow SMI — GPU & system monitoring suite";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    let
      # Every binary the workspace produces, keyed by its actual bin name,
      # each pointing back at the cargo package (crate) that builds it.
      binaries = {
        "plow-smi" = {
          cargoPackage = "plows-cli";
          description = "Unified Plow SMI CLI — export, monitor, and control GPUs from one binary";
        };
        "plows-exporter" = {
          cargoPackage = "plows-exporter";
          description = "Professional GPU metrics exporter for Prometheus (AMD, NVIDIA, Intel, TPU, System)";
        };
        "plows-top" = {
          cargoPackage = "plows-top";
          description = "Professional terminal GPU & system monitor — like htop for GPUs";
        };
        "plows-ctl" = {
          cargoPackage = "plows-ctl";
          description = "GPU power & clock control library & CLI — backed by plows-gpu (NVML / amdgpu sysfs)";
        };
      };
      version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
      homepage = "https://github.com/infervisor/plow-smi";
    in
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        # Keep local build products out of the Cargo input (including when the
        # flake is invoked as path:. to test newly added, untracked files).
        source = pkgs.lib.cleanSourceWith {
          src = self;
          filter = path: type:
            pkgs.lib.cleanSourceFilter path type
            && builtins.baseNameOf path != "target"
            && builtins.baseNameOf path != "result"
            && !(pkgs.lib.hasPrefix "result-" (builtins.baseNameOf path));
        };

        commonMeta = with pkgs.lib; {
          inherit homepage;
          license = licenses.asl20;
          platforms = platforms.linux ++ platforms.darwin;
        };

        # One build compiles the whole workspace; every binary lands in
        # $out/bin. Individual per-binary packages below slice this apart so
        # `nix build .#<name>` never has to recompile the tree from scratch.
        workspace = pkgs.rustPlatform.buildRustPackage {
          pname = "plow-smi-workspace";
          inherit version;
          src = source;
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = [ pkgs.pkg-config ];

          cargoBuildFlags = [ "--workspace" "--bins" ];
          cargoTestFlags = [ "--workspace" ];

          meta = commonMeta // {
            description = "Plow SMI — GPU & system monitoring suite (all binaries)";
          };
        };

        # The same build for hosts where Nix runs under PRoot (nix-portable
        # without user namespaces, e.g. AppArmor-restricted Ubuntu). PRoot
        # cannot run the stock unpack and vendor-copy steps, so this variant
        # unpacks with tar and links the vendored crates instead of copying
        # them. It skips nixpkgs' cargo setup hook and with it the stdenv
        # linker and rustflags config, so it is for local use only: releases
        # and CI build `workspace`.
        workspaceProot = workspace.overrideAttrs (old: {
          pname = "plow-smi-workspace-proot";

          unpackCmd = ''
            dest="$(stripHash "$curSrc")"
            mkdir -p "$dest"
            tar -C "$curSrc" -cf - . | tar -C "$dest" -xf -
          '';

          dontCargoSetupPostUnpack = true;

          postUnpack = ''
            ln -s "$cargoDeps" cargo-vendor-dir
            export cargoDepsCopy="$(realpath cargo-vendor-dir)"
            mkdir -p .cargo
            substitute "$cargoDeps/.cargo/config.toml" .cargo/config.toml \
              --subst-var-by vendor "$cargoDepsCopy"
          '';

          meta = old.meta // {
            description = "Plow SMI — all binaries, built on a PRoot-based Nix (local use)";
          };
        });

        mkBinPackage = name: { cargoPackage, description }:
          pkgs.runCommand name
            {
              meta = commonMeta // {
                inherit description;
                mainProgram = name;
              };
              passthru = { inherit cargoPackage; };
            } ''
            mkdir -p $out/bin
            install -m755 ${workspace}/bin/${name} $out/bin/${name}
          '';

        binPackages = pkgs.lib.mapAttrs mkBinPackage binaries;

        release = import ./nix/release.nix {
          inherit pkgs workspace version system;
          src = source;
        };

        releaseTools = import ./nix/release-tools.nix { inherit pkgs; };

        mkApp = name: pkg: {
          type = "app";
          program = "${pkg}/bin/${name}";
          meta = { description = pkg.meta.description; };
        };
      in
      {
        packages = binPackages // releaseTools.packages // {
          default = binPackages."plow-smi";
          # Full workspace build: every binary in one derivation
          # (`nix build .#all`), useful for images/CI artifact bundling.
          all = workspace;
          # `nix build .#all-proot` on a PRoot-based Nix (see workspaceProot).
          all-proot = workspaceProot;
          # Relocatable archives, usable without Nix installed.
          inherit release;
        };

        apps = (pkgs.lib.mapAttrs mkApp binPackages)
          // (pkgs.lib.mapAttrs (name: pkg: mkApp "plow-smi-${name}" pkg) releaseTools.packages) // {
          default = mkApp "plow-smi" binPackages."plow-smi";
        };

        checks = binPackages // {
          release-ci = releaseTools.check;
          workspace-build-and-test = workspace;
          release-tooling = pkgs.runCommand "plow-smi-release-tooling-tests"
            { nativeBuildInputs = [ pkgs.python3 ]; }
            ''
              export PYTHONDONTWRITEBYTECODE=1
              python3 -m unittest discover -s ${source}/scripts -p 'test_*.py' -v
              touch "$out"
            '';
        };

        devShells.default = pkgs.mkShell {
          buildInputs = [
            pkgs.cargo
            pkgs.rustc
            pkgs.rustfmt
            pkgs.clippy
            pkgs.rust-analyzer
            pkgs.pkg-config
          ];
        };
      }
    ) // {
      nixosModules.default = import ./nix/module.nix self;
      nixosModules.plow-smi-exporter = self.nixosModules.default;

      overlays.default = final: prev: {
        inherit (self.packages.${final.system})
          plow-smi
          plows-exporter
          plows-top
          plows-ctl;
      };
    };
}
