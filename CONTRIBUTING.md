# Contributing

Thanks for contributing to rust-chat. This covers the day-to-day workflow.
Project-specific rules also live in [`AGENTS.md`](AGENTS.md) — read that too.

## Prerequisites

Development assumes the Nix dev shell, which provides the full toolchain
(`rustc`/`cargo`, `just`, `sqlx-cli`, PostgreSQL, Bun and Podman):

```sh
nix develop
```

Run `just --list` to see every recipe.

## Everyday commands

| Task | Command |
| --- | --- |
| Type-check the workspace | `just check` |
| Lint (clippy, warnings are errors) | `just lint` |
| Format / verify formatting | `just fmt` / `just fmt-check` |
| Run the test suite | `just test` |
| Run every CI gate | `just ci` |
| Build optimized binaries | `just build` |
| Run the API server | `just run` |
| Run the sandbox service | `just sandboxd` |

## Frontend

The SvelteKit app lives in `web/` and is driven through Bun:

| Task | Command |
| --- | --- |
| Install dependencies | `just web-install` |
| Dev server | `just web-dev` |
| Type-check | `just web-check` |
| Production build | `just web-build` |

## Database and full stack

| Task | Command |
| --- | --- |
| Start Postgres + pgvector | `just db-up` |
| Apply migrations | `just migrate` |
| Build and start the stack | `just stack-up` |
| Stop the stack / tail logs | `just stack-down` / `just stack-logs` |

The default backend is SQLite; `--profile postgres` opts into Postgres.

## Tests

Do not add new unit or integration tests unless explicitly instructed.
End-to-end and system-level tests are welcome. See [`AGENTS.md`](AGENTS.md).

| Task | Command |
| --- | --- |
| Everything (`cargo test --workspace`) | `just test` |
| End-to-end suites on SQLite | `just e2e` |
| The same suites against Postgres | `just e2e-postgres` |
| Load/soak test (ignored by default) | `just e2e-load` |

The end-to-end suites live in `crates/chat-server/tests/`; they spawn the real
`chat-server` binary (plus a fake provider and a fake `sandboxd`) and drive it
over HTTP/SSE. Postgres cases use `testcontainers` (`pgvector/pgvector:pg16`)
and are skipped when no Docker/Podman runtime is reachable. Setting
`RUSTCHAT_E2E_BACKEND=postgres` runs the *entire* suite against Postgres.

`just e2e-load` opens `RUSTCHAT_LOAD_STREAMS` (default 50, or 4 on SQLite)
concurrent `POST /api/chat` streams, samples the server's `/proc/<pid>/status`
`VmRSS` at peak, and prints the peak RSS and per-stream bytes. It is not part of
blocking CI; record its numbers in the PR body.

## Continuous integration

GitHub Actions runs on every pull request and on pushes to `main`
(`.github/workflows/ci.yml`):

- **Rust** — installs the pinned toolchain from `rust-toolchain.toml`, caches
  cargo, then runs `just ci` (`cargo fmt --check`, `cargo clippy -D warnings`,
  `cargo test --workspace`).
- **Web** — installs Bun and runs `just web-install`, `just web-check` and
  `just web-build`.
- Docker is available on the runner, so the Postgres end-to-end cases in the
  Rust job run via testcontainers.

The load/soak test is not part of blocking CI.

Publishing is manual for now: `.github/workflows/release.yml` only runs on
`workflow_dispatch`. It builds and pushes the `chat-server`, `sandboxd` and
`web` images to GHCR (tagged with the supplied tag and `latest`) and uploads
release artifacts.

See issue #74 (epic #12).

## Style

- Match the surrounding code and existing libraries; prefer editing existing
  files over adding new ones.
- Do not add code comments.

## Commits

- Write an imperative, capitalized subject line (`Add ...`, `Fix ...`,
  `Render ...`), matching the existing history.
- Keep each commit focused on one change.

## Pull requests

- Branch off `main` with a short, kebab-case name (e.g. `m4-computer-egress-allowlist`).
- Run `just ci` (and `just web-check` / `just web-build` for frontend changes)
  before opening the PR.
- Reference the issue in the PR body so it is tracked, ideally so it closes
  automatically:

  ```
  Closes #123
  ```

  `Closes`/`Fixes` auto-close the issue on merge; `Implements #123` does not,
  so the issue has to be closed by hand. Prefer `Closes` when the PR completes
  the issue.
- Include a short **Summary** of the change and how you verified it (the exact
  commands you ran).
