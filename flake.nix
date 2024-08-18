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
        pname = "zingolib-workspace";
        version =
          let
            inherit (builtins) readFile fromTOML;
          in
            (fromTOML (readFile "${src}/zingolib/Cargo.toml")).package.version;

        pkgs = nixpkgs.legacyPackages.${system};

        inherit (pkgs) lib;

        craneLib = crane.mkLib pkgs;

        # TODO: filter source to improve caching / source specificity.
        # Source specificity means selecting only things which impact the resulting build. For example, `README.md` in most projects does not.
        # For now we cast a wide net just to get the build to work.
        src = ./.;

        # See: https://github.com/zcash/librustzcash/issues/1420
        # -and: https://github.com/zingolabs/zingolib/pull/1214#issuecomment-2168047527
        zcbi1420-workaround = pkgs.writeScript "zcbi1420-workaround.sh" ''
          #! ${pkgs.bash}/bin/bash
          set -efuo pipefail

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
          echo "+- Contents of $cargo_config:"
          echo '|'
          sed 's/^/| /' "$cargo_config"
        '';

        # Common arguments can be set here to avoid repeating them later
        # Note: we include system dependencies commonly that only some (transitive) crates require.
        commonArgs = {
          inherit pname version src;

          strictDeps = true;
          doCheck = false;

          # All packages in `nativeBuildInputs` are required at build time but _must not_ be required at runtime:
          nativeBuildInputs = with pkgs; [
            # We include `pkg-config` because `openssl-sys` requires it.
            pkg-config

            # We include git because `zingolib` requires it in its `build.rs` script via the `build_utils` crate.
            git

            # Note that the default package derivation runs nextest, as well as some flake check targets
            cargo-nextest
          ];

          buildInputs = [
            # We include `openssl` lib in all crates, even though it is only necessary for the `openssl-sys` crate.
            pkgs.openssl
          ];

          # Additional environment variables can be set directly
          # MY_CUSTOM_VAR = "some value";
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;
      in
      {
        packages = {
          default = craneLib.buildPackage (commonArgs // {
            inherit cargoArtifacts;

            nativeBuildInputs = commonArgs.nativeBuildInputs ++ [
              pkgs.protobuf
            ];
          });
        };

        apps = {};

        devShells.default = craneLib.devShell {
          inherit cargoArtifacts;

          # Inherit inputs from checks.
          checks = self.checks.${system};

          # Additional dev-shell environment variables can be set directly
          # MY_CUSTOM_DEVELOPMENT_VAR = "something else";

          # Extra inputs can be added here; cargo and rustc are provided by default.
          packages = [
            pkgs.protobuf
          ];
        };

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
            inherit cargoArtifacts pname version src;
          };

          # Audit dependencies
          my-workspace-audit = craneLib.cargoAudit (commonArgs // {
            inherit advisory-db;
          });

          # Audit licenses
          my-workspace-deny = craneLib.cargoDeny {
            inherit cargoArtifacts pname version src;
          };

          # Run tests with cargo-nextest
          # Consider setting `doCheck = false` on other crate derivations
          # if you do not want the tests to run twice
          my-workspace-nextest = craneLib.cargoNextest (commonArgs // {
            inherit cargoArtifacts;

            doCheck = true;
            cargoTestCommand = "cargo nextest run -E'not (package(libtonode-tests) + package(darkside-tests))'";

            partitions = 1;
            partitionType = "count";
          });
        };
      });
}
