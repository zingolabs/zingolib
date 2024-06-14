{
  description = "zingolib workspace";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.rust-analyzer-src.follows = "";
    };

    flake-utils.url = "github:numtide/flake-utils";

    advisory-db = {
      url = "github:rustsec/advisory-db";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, crane, fenix, flake-utils, advisory-db, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        inherit (pkgs) lib;

        craneLib = crane.mkLib pkgs;

        # TODO: filter source to improve caching / source specificity.
        # Source specificity means selecting only things which impact the resulting build. For example, `README.md` in most projects does not.
        # For now we cast a wide net just to get the build to work.
        src = ./.;

        # Common arguments can be set here to avoid repeating them later
        # Note: we include system dependencies commonly that only some (transitive) crates require.
        commonArgs = {
          inherit src;
          strictDeps = true;

          # All packages in `nativeBuildInputs` are required at build time but _must not_ be required at runtime:
          nativeBuildInputs = with pkgs; [
            # We include `pkg-config` because `openssl-sys` requires it.
            pkg-config

            # We include git because `zingolib` requires it in its `build.rs` script via the `build_utils` crate.
            git

            # We include `protobuf` because `darkside-tests` requires it.
            protobuf
          ];

          buildInputs = [
            # We include `openssl` lib in all crates, even though it is only necessary for the `openssl-sys` crate.
            pkgs.openssl
          ] ++ lib.optionals pkgs.stdenv.isDarwin [
            # Additional darwin specific inputs can be set here
            pkgs.libiconv
          ];

          # Additional environment variables can be set directly
          # MY_CUSTOM_VAR = "some value";

          # See: https://github.com/zcash/librustzcash/issues/1420
          # -and: https://github.com/zingolabs/zingolib/pull/1214#issuecomment-2168047527
          buildPhaseCargoCommand = pkgs.writeScript "zcbi1420-workaround.sh" ''
            #! ${pkgs.bash}/bin/bash
            set -efuxo pipefail

            cargo_config='.cargo-home/config.toml'

            function get_nix_store_cargo_prereqs {
              grep '^directory' "$cargo_config" \
                | sed 's/^directory = "//; s/"$//'
            }

            mkdir ./rw_dependencies
            for store_src in $(get_nix_store_cargo_prereqs)
            do
              rw_src_name="$(echo "$store_src" | tr '/' '_')"
              rw_src="./rw_dependencies/$rw_src_name"
              cp -r --dereference "$store_src" "$rw_src"
              chmod -R u+w "$rw_src"
              sed -i "s|$store_src|$rw_src|" "$cargo_config"
            done

            # For diagnostics (not necessary for build):
            cat "$cargo_config"

            set +x
          '';
        };

        craneLibLLvmTools = craneLib.overrideToolchain
          (fenix.packages.${system}.complete.withComponents [
            "cargo"
            "llvm-tools"
            "rustc"
          ]);

        # Build *just* the cargo dependencies (of the entire workspace),
        # so we can reuse all of that work (e.g. via cachix) when running in CI
        # It is *highly* recommended to use something like cargo-hakari to avoid
        # cache misses when building individual top-level-crates
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;
      in
      {
        checks = {
          # Run clippy (and deny all warnings) on the workspace source,
          # again, reusing the dependency artifacts from above.
          #
          # Note that this is done as a separate derivation so that
          # we can block the CI if there are issues here, but not
          # prevent downstream consumers from building our crate by itself.
          my-workspace-clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--all-targets -- --deny warnings";
          });

          my-workspace-doc = craneLib.cargoDoc (commonArgs // {
            inherit cargoArtifacts;
          });

          # Check formatting
          my-workspace-fmt = craneLib.cargoFmt {
            inherit src;
          };

          # Audit dependencies
          my-workspace-audit = craneLib.cargoAudit {
            inherit src advisory-db;
          };

          # Audit licenses
          my-workspace-deny = craneLib.cargoDeny {
            inherit src;
          };

          # Run tests with cargo-nextest
          # Consider setting `doCheck = false` on other crate derivations
          # if you do not want the tests to run twice
          my-workspace-nextest = craneLib.cargoNextest (commonArgs // {
            inherit cargoArtifacts;
            partitions = 1;
            partitionType = "count";
          });

          # Ensure that cargo-hakari is up to date
          my-workspace-hakari = craneLib.mkCargoDerivation {
            inherit src;
            pname = "my-workspace-hakari";
            cargoArtifacts = null;
            doInstallCargoArtifacts = false;

            buildPhaseCargoCommand = ''
              cargo hakari generate --diff  # workspace-hack Cargo.toml is up-to-date
              cargo hakari manage-deps --dry-run  # all workspace crates depend on workspace-hack
              cargo hakari verify
            '';

            nativeBuildInputs = [
              pkgs.cargo-hakari
            ];
          };
        };

        packages = {
          default = craneLib.buildPackage (commonArgs // { inherit cargoArtifacts; });

          my-workspace-llvm-coverage = craneLibLLvmTools.cargoLlvmCov (commonArgs // {
            inherit cargoArtifacts;
          });
        };

        apps = {};

        devShells.default = craneLib.devShell {
          # Inherit inputs from checks.
          checks = self.checks.${system};

          # Additional dev-shell environment variables can be set directly
          # MY_CUSTOM_DEVELOPMENT_VAR = "something else";

          # Extra inputs can be added here; cargo and rustc are provided by default.
          packages = [
            pkgs.cargo-hakari
          ];
        };
      });
}
