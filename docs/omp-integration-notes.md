# Oh My Pi notes

Facts verified against the installed OMP `18.15.0` on Windows. This file exists
so a future change does not have to rediscover them, and so the difference
between "documented upstream" and "measured here" stays visible.

## Paths

| Thing | Value |
| --- | --- |
| Default agent directory | `%USERPROFILE%\.omp\agent` |
| Named profile | `%USERPROFILE%\.omp\profiles\<name>\agent` |
| Provider registry | `models.yml`, falling back to `models.yaml` |
| Roles and settings | `config.yml`, falling back to `config.yaml` |

`PI_CODING_AGENT_DIR` overrides the default agent directory. It does **not**
select a named profile. `OMP_PROFILE` (legacy `PI_PROFILE`) selects one.

An absolute `PI_CONFIG_DIR` was observed to be joined under the user profile,
producing a path such as `C:\Users\name\C:\Users\name\...`. Eclink therefore
uses `PI_CODING_AGENT_DIR` for verification and does not depend on
`PI_CONFIG_DIR`.

## Provider schema

```yaml
providers:
  provider-id:
    baseUrl: https://host/v1
    api: openai-completions
    apiKey: '!"C:\path\eclink-helper.exe" secret resolve <profile-id>'
    authHeader: true
    models:
      - id: exact-model-id
        name: Display name
        contextWindow: 128000
        maxTokens: 16384
```

`api` accepts `openai-completions`, `openai-responses`, and
`anthropic-messages`.

The `apiKey` value is a YAML string whose content begins with `!`. The whole
value must be quoted; an unquoted `!` starts a tag and makes the file invalid.
Single quotes are the safe spelling on Windows because the command contains a
quoted executable path with backslashes.

OMP runs the command through its own shell with a 10 second timeout, trims the
stdout, and caches the result. The helper command is therefore the primary
credential path, not an environment fallback.

## Roles

`config.yml` uses `modelRoles`:

```yaml
modelRoles:
  default: provider/model
  smol: provider/model
  slow: provider/model
  plan: provider/model
```

Selectors are `provider/model`, optionally with `:thinking`. `omp config get
modelRoles --json` works; the nested form `omp config get modelRoles.default`
returns `Unknown setting`.

## CLI checks used by Eclink

| Purpose | Command |
| --- | --- |
| Version gate | `omp --version` |
| Effective config | `omp config path` |
| Registry validation | `omp models --json` |
| Role readback | `omp config get modelRoles --json` |

`omp models --json` exits successfully and lists the provider only when
`models.yml` parsed. Eclink uses that as the post-write verification and rolls
both files back if it fails.

## Launch

`omp --profile <name> --model <provider>/<model> --cwd <dir>` was accepted with
space-separated values. Eclink passes argv directly, never a joined shell
string.

## Profile names

OMP accepts `^[a-z0-9][a-z0-9._-]{0,63}$`, excluding `default` (the reserved
default profile) and Windows device names such as `con` and `com1`.

## Backups

OMP permits literal credentials in `models.yml`. Eclink seals OMP snapshots and
originals with Windows DPAPI rather than writing a second plaintext copy. The
sealed payload is still readable by the same Windows user, so Restore Original
remains exact.