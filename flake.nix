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
            # DB_BACKEND / DATABASE_URL are read from .env (see .env.example),
            # so the local backend is chosen in one place. Do not export
            # DATABASE_URL here: dotenvy does not override the environment, so a
            # value set here would shadow .env.
            echo "rust-chat dev shell"
            echo "  rustc: $(rustc --version 2>/dev/null || echo 'n/a')"
            echo "  db: backend from .env (.env.example defaults to sqlite)"
            echo "  start postgres: just db-up   ·   cargo check: cargo check"
          '';
        };
      });

      # Convenience formatter
      formatter = forAll (pkgs: pkgs.nixpkgs-fmt);
    };
}
