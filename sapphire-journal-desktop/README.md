# sapphire-journal-desktop

Cross-platform desktop GUI for [sapphire-journal](https://github.com/fluo10/sapphire-journal) — a Markdown-based task and note manager. Built with [egui](https://github.com/emilk/egui) / eframe.

The desktop app is the primary human-facing UI: it manages a registry of journals stored under your user data directory, embeds an HTTP MCP server for AI-agent integrations, and provides an editor, tree view, and settings panel for working with entries.

## Installation

The desktop crate is `publish = false`, so prebuilt binaries from the [Releases](https://github.com/fluo10/sapphire-journal/releases) page are the supported install path.

### Install script (Linux / macOS)

```sh
curl -fsSL https://raw.githubusercontent.com/fluo10/sapphire-journal/main/sapphire-journal-desktop/install.sh | sh
```

### Install script (Windows)

```powershell
irm https://raw.githubusercontent.com/fluo10/sapphire-journal/main/sapphire-journal-desktop/install.ps1 | iex
```

Both scripts download the latest `desktop-v*` release asset for your platform into `~/.local/bin/` (Windows: `%USERPROFILE%\.local\bin\`) and add the directory to `PATH` if needed.

### From source

```sh
cargo build --release --package sapphire-journal-desktop
# binary at target/release/sapphire-journal-desktop
```

On Linux, building requires the GTK/WebKit dev packages:

```sh
sudo apt-get install protobuf-compiler libgtk-3-dev libwebkit2gtk-4.1-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev libxdo-dev
```

### Uninstall

Remove the installed binary:

```sh
# Linux / macOS (install script location)
rm ~/.local/bin/sapphire-journal-desktop

# Windows (PowerShell, install script location)
Remove-Item "$HOME\.local\bin\sapphire-journal-desktop.exe"
```

The app keeps the journal registry and managed journal repositories under `$XDG_DATA_HOME/sapphire-journal/` (defaults to `~/.local/share/sapphire-journal/` on Linux / macOS). This contains `journals.toml` (the registry), `settings.toml`, and `journals/<uuid>/` (the actual entry files for each managed journal). **Deleting it removes your data.** Back up or git-push first if you want to keep anything.

## Data layout

```
$XDG_DATA_HOME/sapphire-journal/
├── journals.toml          # list of registered journals
├── settings.toml          # UI preferences (last-opened, MCP server, etc.)
└── journals/
    └── <uuid>/            # per-journal git repository
        ├── .sapphire-journal/
        └── 2026/...
```

Journals registered through the GUI are git repositories on disk — you can `cd` into them and use `git`, sync them with another machine, or open them with the CLI directly.

## MCP server

The desktop app embeds the [`sapphire-journal-mcp`](../sapphire-journal-mcp/) HTTP server. Enable it in **Settings → MCP** to let AI agents (Claude, etc.) read and write the currently open journal over HTTP.

## License

GPL-3.0-or-later, with an additional [App Store / marketplace compatibility exception](LICENSE.App-Store-Exception) that permits distribution through official application marketplaces (Apple App Store, Google Play, Microsoft Store, etc.) whose standard terms would otherwise conflict with GPL §6/§7.

See [LICENSE](LICENSE) for the full GPL-3.0 text.
