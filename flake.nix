{
  description = "rust-chat — a lean, Rust + Postgres reimplementation of a LibreChat-style AI chat platform";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAll = f: nixpkgs.lib.genAttrs systems (s: f nixpkgs.legacyPackages.${s});
    in
    {
      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            # Rust toolchain
            rustc
            cargo
            rustfmt
            clippy
            rust-analyzer
            # DB tooling
            sqlx-cli
            postgresql_16
            # Frontend
            nodejs_22
            bun
            # Container runtime used by `podman compose` for the local stack
            podman
            # Misc
            just
            git
            jq
          ];

          shellHook = ''
            export PGDATA="$PWD/.dev/pgdata"
            export DATABASE_URL="''${DATABASE_URL:-postgres://rustchat:rustchat@localhost:5432/rustchat}"
            echo "rust-chat dev shell"
            echo "  rustc: $(rustc --version 2>/dev/null || echo 'n/a')"
            echo "  DATABASE_URL=$DATABASE_URL"
            echo "  start db: just db-up   ·   cargo check: cargo check"
          '';
        };
      });

      # Convenience formatter
      formatter = forAll (pkgs: pkgs.nixpkgs-fmt);
    };
}
