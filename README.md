<p align="center">
  <img src="logos/logo-with-text.svg" alt="Doplarr" width="480">
</p>

<p align="center">
  <a href="https://github.com/kiranshila/doplarr_rs/actions/workflows/ci.yml"><img alt="CI" src="https://img.shields.io/github/actions/workflow/status/kiranshila/doplarr_rs/ci.yml?style=for-the-badge"></a>
  <a href="https://discord.gg/890634173751119882"><img alt="Discord" src="https://img.shields.io/discord/890634173751119882?color=ff69b4&label=discord&style=for-the-badge"></a>
  <a href="LICENSE-MIT"><img alt="License" src="https://img.shields.io/badge/license-MIT%2FApache--2.0-blue?style=for-the-badge"></a>
</p>

A modern Discord bot for requesting media through \*arr backends, written in Rust.

## Overview

Doplarr is a Discord bot that allows users to request media from \*arr backends through slash commands.
It integrates seamlessly with your *arr stack (Sonarr, Radarr) to automate media requests from Discord.

This is a **complete rewrite** of the [original Doplarr](https://github.com/kiranshila/Doplarr) (written in Clojure) in Rust, offering improved performance, reduced resource usage, and easier deployment.

### Key Features

- **Modern Discord UI**: Built with Discord's V2 Components for a polished, native-looking interface
- **Slash Commands**: Modern Discord interactions - no message content access needed
- **Flexible Backend Configuration**: Define multiple slash commands per backend type - use different instances or the same instance with different settings (e.g., `/request movie` and `/request movie_4k` with separate quality profiles/root folders)
- **No Privileged Intents Required**: Minimal permissions for maximum privacy
- **Lightweight**: Minimal resource footprint with fast startup times
- **Simple Configuration**: Single TOML config file with sensible defaults
- **Docker Support**: Pre-built containers available via GitHub Container Registry
- **Extensible**: Clean architecture for adding additional *arr backends

## Screenshots

<p align="center">
  <img src="screenshots/series.png" alt="Series selection interface" width="400">
</p>

## What's Different from the Original?

If you're migrating from the Clojure version of Doplarr:

| Feature | Original (Clojure) | This Version (Rust) |
|---------|-------------------|---------------------|
| **Discord UI** | V1 Components | V2 Components (richer layouts) |
| **Backend Flexibility** | One instance per backend type | Multiple configurations per backend (e.g., `movie` + `movie_4k`) |
| **Configuration** | EDN file or env vars | TOML with validation & helpful errors |
| **Configuration Options** | Basic settings | More robust options (series type: Standard/Anime/Daily, monitor types, minimum availability, etc.) |
| **Logging** | Basic logging | Structured logging with granular levels |
| **Runtime** | Requires Java 11+ | Native binary |
| **Resource Usage** | JVM overhead | Native binary (lightweight) |
| **Startup Time** | Typical JVM startup | Near-instant |

### Migration Notes

- **Configuration format changed**: You'll need to convert EDN/environment variables to TOML format (see [Configuration](#configuration))
  - TOML config provides validation with clear error messages
  - Type-safe configuration catches mistakes at startup
- **Same Discord commands**: The user experience is identical - all slash commands work the same way

## Installation

### Prerequisites

1. **Discord Bot Token**
   - Go to the [Discord Developer Portal](https://discord.com/developers/applications)
   - Create a new application
   - Go to the "Bot" section and create a bot
   - Copy the bot token (you'll need this for configuration)
   - Under "OAuth2" → "URL Generator":
     - Select scopes: `bot`, `applications.commands`
     - Select bot permissions: `Send Messages` (required if `public_followup` is enabled)
   - Use the generated URL to invite the bot to your server

2. **Sonarr and/or Radarr**
   - At least one backend is required
   - Get your API key from Settings → General → Security

### Docker (Recommended)

The easiest way to run Doplarr is using Docker:

```bash
docker run -d \
  --name doplarr \
  --restart unless-stopped \
  -v /path/to/config.toml:/config.toml:ro \
  ghcr.io/kiranshila/doplarr_rs:latest
```

Or using Docker Compose:

```yaml
services:
  doplarr:
    image: ghcr.io/kiranshila/doplarr_rs:latest
    container_name: doplarr
    restart: unless-stopped
    volumes:
      - ./config.toml:/config.toml:ro
```

### Building from Source

#### Using Nix (Recommended)

If you have [Nix](https://determinate.systems/nix-installer/) with flakes enabled:

```bash
# Build the binary
nix build

# Run directly
nix run . /path/to/config.toml

# Build the Docker image
nix build .#dockerImage
docker load < result
```

#### Using Cargo

Requirements:
- [Rust](https://rustup.rs/)
- OpenSSL development libraries (Linux)
- pkg-config (Linux)

```bash
# Clone the repository
git clone https://github.com/kiranshila/doplarr_rs.git
cd doplarr_rs

# Build release binary
cargo build --release

# Run
./target/release/doplarr /path/to/config.toml
```

## Configuration

Create a `config.toml` file with your settings. See [config.example.toml](config.example.toml) for a complete example.

```toml
# Required: Your Discord bot token
discord_token = "YOUR_DISCORD_BOT_TOKEN"

# Optional: Logging level (default: "info")
# Format: "target=level" or just "level"
# Levels: error, warn, info, debug, trace
log_level = "doplarr=info"

# Optional: Make follow-up messages public (default: true)
# Set to false to keep all bot responses ephemeral (only visible to requester)
# Note: Requires "Send Messages" permission in Discord when enabled
public_followup = true

# ============================================================================
# BACKENDS
# ============================================================================
# Each backend creates a slash command: /request <media>
# You can define multiple backends of the same type with different settings.
# The "media" field becomes the subcommand name (e.g., "movie" -> /request movie)

# Standard movie requests
[[backends]]
media = "movie"

[backends.config.Radarr]
url = "http://localhost:7878"
api_key = "your_radarr_api_key"
# Optional settings - if omitted, users select at runtime
quality_profile = "HD-1080p"
rootfolder = "/movies"
monitor_type = "movieOnly"           # movieOnly, movieAndCollection, none
minimum_availability = "announced"   # announced, inCinemas, released

# 4K movie requests (same or different Radarr instance)
[[backends]]
media = "movie_4k"

[backends.config.Radarr]
url = "http://localhost:7878"        # Can be same instance...
api_key = "your_radarr_api_key"
quality_profile = "Ultra-HD"         # ...with different quality profile
rootfolder = "/movies/4k"            # ...and different root folder

# TV series requests
[[backends]]
media = "series"

[backends.config.Sonarr]
url = "http://localhost:8989"
api_key = "your_sonarr_api_key"
quality_profile = "WEB-1080p"
rootfolder = "/tv"
season_folders = true
series_type = "standard"             # standard, daily, anime
# Restrict which monitor options users can select
allowed_monitor_types = ["firstSeason", "lastSeason", "latestSeason", "pilot", "recent"]

# Anime requests (separate Sonarr instance or same with different settings)
[[backends]]
media = "anime"

[backends.config.Sonarr]
url = "http://localhost:8990"        # Could be separate Sonarr for anime
api_key = "your_anime_sonarr_api_key"
series_type = "anime"
rootfolder = "/anime"
```

### Configuration Tips

- **At least one backend required**: You must configure at least one `[[backends]]` entry
- **Unique media names**: Each backend must have a unique `media` value (this becomes the slash command)
- **Optional settings**: When optional settings (quality_profile, rootfolder, monitor_type, etc.) are not specified, users select them at runtime through the Discord interface
- **Quality profiles**: Use the exact name as shown in Sonarr/Radarr settings
- **Root folders**: Must be a path that exists in your *arr configuration
- **Monitor types for Sonarr**: The `allowed_monitor_types` setting restricts user choices, preventing selections like "all" which might download too much
- **Multiple configurations**: You can point multiple backends at the same *arr instance with different settings (e.g., different quality profiles for 4K vs standard)

## Running as a Service

### systemd (Linux)

Create `/etc/systemd/system/doplarr.service`:

```ini
[Unit]
Description=Doplarr Discord Bot
After=network.target

[Service]
Type=simple
User=doplarr
Group=doplarr
WorkingDirectory=/opt/doplarr
ExecStart=/opt/doplarr/doplarr /opt/doplarr/config.toml
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

Enable and start:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now doplarr
```

### Docker Compose with Auto-restart

The `restart: unless-stopped` policy ensures the bot automatically restarts on failure or system reboot.

## Logging

Logging is configured via the `log_level` setting in your config file. You can set different levels for different components:

```toml
# Just set overall level
log_level = "info"

# Or be more specific
log_level = "doplarr=debug,twilight_gateway=warn"
```

Logs include:
- Connection status to Discord
- Search requests and results
- Backend API calls and responses
- Error details (with sanitized user-facing messages)
- Performance metrics

## Troubleshooting

### Bot not responding to commands

1. **Check bot was invited correctly**: Ensure you used the OAuth2 URL with `bot` and `applications.commands` scopes
2. **Check logs**: Look for connection errors or API issues
3. **Verify token**: Make sure your Discord token is correct
4. **Commands not showing**: Commands register automatically on startup; wait 1-2 minutes or restart Discord

### Backend connection errors

1. **Check URLs**: Ensure Sonarr/Radarr URLs are accessible from where Doplarr is running
2. **Verify API keys**: Test your API keys using the *arr web interface
3. **Network issues**: If using Docker, ensure containers can reach your *arr services
4. **SSL certificates**: Docker image includes CA certificates; if issues persist, check your SSL setup

### Configuration errors

1. **TOML syntax**: Use a TOML validator if you get parse errors
2. **Quality profiles**: Must match exactly (case-sensitive) what's in your *arr settings
3. **Root folders**: Must be paths that exist in *arr configuration
4. **Required fields**: `discord_token` and at least one `[[backends]]` entry are required
5. **Duplicate media names**: Each backend must have a unique `media` value

## Development

See [README_DEVELOPER.md](README_DEVELOPER.md) for information on:
- Adding new backend providers
- Generating API bindings from OpenAPI specs
- Contributing to the project

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.

## Acknowledgments

- [Twilight](https://github.com/twilight-rs/twilight) for Discord API bindings
- [OpenAPI Generator](https://github.com/OpenAPITools/openapi-generator) for *arr API clients
