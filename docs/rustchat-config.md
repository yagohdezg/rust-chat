# rustchat.yaml configuration

`rustchat.yaml` is an optional, declarative deployment config for `chat-server`.
Its path comes from `RUSTCHAT_CONFIG` and defaults to
`/etc/rustchat/rustchat.yaml`; a missing file is not an error.

The file is a read-only, global **base layer**. It is meant to be baked into an
image or mounted from a ConfigMap, so it must never contain secrets — provider
API keys keep resolving from the DB/env via `SECRET_ENCRYPTION_KEY`. Environment
variables and runtime/DB state always override it.

Start from [`rustchat.yaml.example`](../rustchat.yaml.example).

## Base-layer rule

For every setting that also exists as an environment variable, the value is
resolved in this order:

1. the environment variable, when it is set and non-empty;
2. the value from `rustchat.yaml`, when present;
3. the built-in default.

So a deployment with no config file behaves exactly as if only environment
variables existed. This is why the file is safe to add later: it only supplies
values that are not otherwise set.

## `${VAR}` interpolation

Every string scalar may reference the environment with `${VAR}` or
`${VAR:-default}`. An unset variable without a default fails boot with a clear
error. Interpolation applies to values only; keys are left untouched, and the
file is expected to hold no secrets.

```yaml
providers:
  - name: local
    kind: custom
    base_url: ${OLLAMA_BASE_URL:-http://localhost:11434/v1}
```

## Unknown keys fail fast

Every section is parsed with unknown-key rejection, so a typo in a key (at the
top level or inside a section) fails boot with an error naming the file and the
offending key, instead of being silently ignored.

## Sections

### `providers`

Globally-declared providers every user inherits (personal providers are still
created through the API). Synced into the store on boot.

| Key | Meaning |
| --- | --- |
| `name` | Unique name referenced by `defaults` / `title_model`. |
| `kind` | `openai`, `anthropic` or `custom`. |
| `base_url` | Provider API base URL. |
| `models` | Models the provider advertises; omit to discover at runtime. |

No environment variable overrides this section.

### `defaults`

Base `provider` + `model` used when nothing else names one. Must reference a
provider declared above and, when that provider lists models, one of its models.

### `title_model`

`provider` + `model` used to auto-generate conversation titles from the first
exchange. Omit to disable title generation.

### `interface`

UI copy and outbound links served to the web app. Parsed and validated; no
consumer yet.

| Key | Overriding env var |
| --- | --- |
| `custom_welcome` | — |
| `privacy_policy` | — |
| `terms_of_service` | — |

### `registration`

Self-service sign-up policy, enforced by `POST /api/auth/register`.

| Key | Env var | Default |
| --- | --- | --- |
| `enabled` | `REGISTRATION_ENABLED` | `true` |
| `allowed_domains` | `REGISTRATION_ALLOWED_DOMAINS` | empty (any domain) |

`allowed_domains` is a list in the file and a comma-separated list in the env
var. When set, the domain part of the email (after the last `@`, compared
case-insensitively) must be in the list. With registration disabled the route
returns `403`; a non-matching domain returns `400` (`email domain is not
allowed`).

### `rate_limits`

Per-IP request limits for the auth and chat routes.

| Key | Env var | Default |
| --- | --- | --- |
| `enabled` | `RATE_LIMIT_ENABLED` | `true` |
| `auth_per_minute` | `RATE_LIMIT_AUTH_PER_MINUTE` | `30` |
| `chat_per_minute` | `RATE_LIMIT_CHAT_PER_MINUTE` | `60` |

### `file`

Upload and sandbox-output size and type limits.

| Key | Env var | Default |
| --- | --- | --- |
| `max_bytes` | `UPLOAD_MAX_BYTES` | `26214400` |
| `output_max_bytes` | `FILE_OUTPUT_MAX_BYTES` | `8388608` |
| `output_allowed_types` | `FILE_OUTPUT_ALLOWED_TYPES` | empty (all types) |

`output_allowed_types` entries may be a full MIME (`image/png`), a wildcard
(`image/*`) or a bare extension (`png`). The file takes a list; the env var
takes a comma-separated list.

### `speech`

Speech (text-to-speech / speech-to-text) toggle. Parsed and validated; no
consumer yet.

| Key | Overriding env var |
| --- | --- |
| `enabled` | — |

## Example

```yaml
providers:
  - name: openai
    kind: openai
    base_url: https://api.openai.com/v1
    models:
      - gpt-4o
      - gpt-4o-mini

defaults:
  provider: openai
  model: gpt-4o-mini

interface:
  custom_welcome: "Welcome! Ask me anything."

registration:
  enabled: true
  allowed_domains:
    - example.com

rate_limits:
  enabled: true
  auth_per_minute: 30
  chat_per_minute: 60

file:
  max_bytes: 26214400
  output_max_bytes: 8388608
  output_allowed_types:
    - image/png
    - application/pdf

speech:
  enabled: false
```
