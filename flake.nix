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

        # Bump the workspace version, the way plow and inferos do:
        #   nix run .#bump            0.1.0 -> 0.1.1
        #   nix run .#bump -- 1.0.0   exactly 1.0.0
        # Run from the repo root: the workspace version in Cargo.toml and the
        # workspace crates' Cargo.lock entries. CI runs it on every release.
        bump = pkgs.writeShellApplication {
          name = "plow-smi-bump";
          runtimeInputs = [ pkgs.coreutils pkgs.gnused pkgs.gawk pkgs.gnugrep ];
          text = ''
            die() { echo "bump: $*" >&2; exit 1; }

            [ -f Cargo.lock ] && [ -f Cargo.toml ] || die "run this from the plow-smi repo root"
            [ "$#" -le 1 ] || die "usage: nix run .#bump [-- X.Y.Z]"

            semver() { [[ "$1" =~ ^(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$ ]]; }

            current="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' Cargo.toml)"
            semver "$current" || die "Cargo.toml workspace version is not X.Y.Z (found '$current')"

            IFS=. read -r major minor patch <<<"$current"
            if [ "$#" -eq 0 ]; then
              next="$major.$minor.$((patch + 1))"
            else
              next="$1"
              semver "$next" || die "'$next' is not MAJOR.MINOR.PATCH"
            fi

            names=()
            for m in $(sed -n '/^members/,/^]/p' Cargo.toml | grep -o '"[^"]*"' | tr -d '"'); do
              names+=("$(sed -n 's/^name = "\(.*\)"$/\1/p' "$m/Cargo.toml" | head -n1)")
            done

            sed -i "0,/^version = \"$current\"$/s//version = \"$next\"/" Cargo.toml
            awk -v names=" ''${names[*]} " -v from="$current" -v to="$next" '
              /^name = "/ { n = $3; gsub(/"/, "", n); pending = index(names, " " n " ") > 0 }
              pending && $0 == "version = \"" from "\"" { $0 = "version = \"" to "\""; pending = 0 }
              { print }
            ' Cargo.lock > Cargo.lock.new && mv Cargo.lock.new Cargo.lock

            echo "plow-smi $current -> $next"
          '';
        };

        mkApp = name: pkg: {
          type = "app";
          program = "${pkg}/bin/${name}";
          meta = { description = pkg.meta.description; };
        };
      in
      {
        packages = binPackages // {
          default = binPackages."plow-smi";
          # Full workspace build: every binary in one derivation
          # (`nix build .#all`), useful for images/CI artifact bundling.
          all = workspace;
          # Relocatable archives, usable without Nix installed.
          inherit release;
          inherit bump;
        };

        apps = (pkgs.lib.mapAttrs mkApp binPackages) // {
          default = mkApp "plow-smi" binPackages."plow-smi";
          bump = {
            type = "app";
            program = "${bump}/bin/plow-smi-bump";
            meta = { description = "Bump the workspace version"; };
          };
        };

        checks = binPackages // {
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
