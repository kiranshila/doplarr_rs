# Migrating from Doplarr (Clojure) to Doplarr (Rust)

The Rust version is a complete rewrite.
It has a new config format, new Docker image, and a few renamed/removed options.
This doc maps everything old to its new equivalent.

## Config format: EDN → TOML

The old bot read an EDN file with namespaced keys. The new bot reads a TOML file where each backend is its own `[[backends]]` table.

**Before (config.edn):**
```edn
{:sonarr/url "http://localhost:8989"
 :sonarr/api "your_sonarr_api_key"
 :radarr/url "http://localhost:7878"
 :radarr/api "your_radarr_api_key"
 :discord/token "your_discord_token"}
```

**After (config.toml):**
```toml
discord_token = "your_discord_token"

[[backends]]
media = "series"
[backends.config.Sonarr]
url = "http://localhost:8989"
api_key = "your_sonarr_api_key"

[[backends]]
media = "movie"
[backends.config.Radarr]
url = "http://localhost:7878"
api_key = "your_radarr_api_key"
```

The `media` field sets the name of the `/request <media>` slash command. You can name it anything you want — `series`, `tv`, `movie`, `film`, etc.

## Environment variables (no config file)

If you ran the Clojure bot with **only environment variables and no mounted config**, that keeps working — the Rust bot detects the same legacy variables on startup, builds a config from them, and runs. No config file or volume required.

| Setting | Variable |
|---|---|
| Discord token | `DISCORD__TOKEN` |
| Seerr / Overseerr | `OVERSEERR__URL`, `OVERSEERR__API`, `OVERSEERR__DEFAULT_ID` |
| Sonarr | `SONARR__URL`, `SONARR__API` |
| Radarr | `RADARR__URL`, `RADARR__API` |
| Log level | `LOG_LEVEL` |

Only connection settings (URL/API, plus the Seerr fallback user) are read from the environment; per-backend options like quality profiles are no longer prompted via env vars — set them by mounting a config file. Doplarr writes the generated `config.toml` (wired to the variables above via `${...}`) when it can, so mounting a volume lets you keep and extend it.

**Overseerr generates two commands.** Mirroring the Clojure bot, `OVERSEERR__*` produces separate `movie` and `series` commands (two `[[backends]]` entries with `media_filter = "movie"` / `media_filter = "tv"`). Because Overseerr fronts Sonarr/Radarr, it takes precedence: if `OVERSEERR__*` is set, the `SONARR__*`/`RADARR__*` variables are ignored (with a note logged at startup). Set up the direct `SONARR__*`/`RADARR__*` backends only when you're not using Overseerr.

You can also reference environment variables from anywhere in a config file with `${VAR}`:

```toml
[backends.config.Seerr]
url = "${OVERSEERR__URL}"
api_key = "${OVERSEERR__API}"
```

## Option mapping

### Global

| Old key | New key | Notes |
|---|---|---|
| `:discord/token` | `discord_token` | Top-level string |
| `:log-level` | `log_level` | String instead of keyword — e.g. `"info"` instead of `:info` |
| `:discord/requested-msg-style` | `public_followup` | `:none` → `false`; `:plain` or `:embed` → `true` (default). The embed/plain distinction is gone. |
| `:discord/max-results` | *(removed)* | Fixed at Discord's 25-item autocomplete limit |

### Sonarr

| Old key | New key | Notes |
|---|---|---|
| `:sonarr/url` | `url` | Under `[backends.config.Sonarr]` |
| `:sonarr/api` | `api_key` | Renamed from `api` |
| `:sonarr/quality-profile` | `quality_profile` | Optional; prompts user if omitted |
| `:sonarr/rootfolder` | `rootfolder` | Optional; prompts user if omitted |
| `:sonarr/season-folders` | `season_folders` | Optional |
| `:sonarr/language-profile` | *(removed)* | Sonarr v4 dropped language profiles |
| `:partial-seasons` | *(removed)* | The season selection UI no longer offers a partial-season flow |

### Radarr

| Old key | New key | Notes |
|---|---|---|
| `:radarr/url` | `url` | Under `[backends.config.Radarr]` |
| `:radarr/api` | `api_key` | Renamed from `api` |
| `:radarr/quality-profile` | `quality_profile` | Optional; prompts user if omitted |
| `:radarr/rootfolder` | `rootfolder` | Optional; prompts user if omitted |

### Overseerr → Seerr

The backend has been moved to `Seerr` (covers both Overseerr and Jellyseerr):

| Old key | New key | Notes |
|---|---|---|
| `:overseerr/url` | `url` | Under `[backends.config.Seerr]` |
| `:overseerr/api` | `api_key` | Renamed from `api` |
| `:overseerr/default-id` | `fallback_user_id` | Same semantics — Seerr user ID for unlinked Discord users |

## New options

These have no equivalent in the Clojure version:

| Key | Backend | Description |
|---|---|---|
| `monitor_type` | Radarr | Lock all requests to a specific monitor mode instead of prompting |
| `minimum_availability` | Radarr | Pre-set minimum availability instead of prompting |
| `series_type` | Sonarr | Force `standard`, `daily`, or `anime`; omit to auto-detect from genres |
| `allow_specials` | Sonarr | Offer Season 0 in the season picker |
| `allow_all_seasons` | Sonarr, Seerr | Offer an "All Seasons" option (all current + future seasons); default true |
| `allow_4k` | Seerr | Show a Standard/4K quality choice at request time |

You can also point multiple `[[backends]]` entries at the same Radarr or Sonarr instance with different settings to create separate commands — e.g. `/request movie` and `/request movie_4k` from one Radarr instance.

The new image reads config from `/config.toml`. Update your volume mount:

```yaml
services:
  doplarr:
    image: ghcr.io/activexray/doplarr_rs:latest
    container_name: doplarr
    restart: unless-stopped
    volumes:
      - ./config.toml:/config.toml:ro
```

See [config.example.toml](config.example.toml) for a full annotated reference.
