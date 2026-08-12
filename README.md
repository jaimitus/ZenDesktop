# 🧘 ZenDesktop

> Dynamic desktop organizer for Windows. Ultralight, native, and free.
> A modern open-source alternative to Stardock Fences.

<p align="center">
  <img src="screenshot.png" alt="ZenDesktop" width="49%">
  <img src="screenshot-2.png" alt="ZenDesktop" width="49%">
</p>

[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11-blue.svg)]()
[![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)
[![Release](https://img.shields.io/github/v/release/jaimitus/ZenDesktop)](https://github.com/jaimitus/ZenDesktop/releases)

## ✨ Features

- **🪟 Floating translucent fences** — Group desktop files into elegant boxes with real alpha transparency
- **📑 Tabbed fences** — Group multiple fences under tabs within a single window to save desktop space
- **🔼 Roll-up fences** — Double click a fence's title bar to instantly roll it up, leaving only the title visible
- **🎯 Smart Drag & Drop** — Drag files between fences, to subfolders, or to the desktop with visual feedback
- **📝 F2 Rename** — Press F2 on any item to directly rename it in place
- **🤖 Auto-organization** — Rules by extension, name, pattern, or local AI (Ollama) for automatic file classification
- **📦 Explorer drop** — Drop files from any folder directly into fences (native OLE drag & drop)
- **🔍 Integrated search** — Filter files within each fence in real time
- **📐 Grid + List view** — Grid or list mode, sortable by name, size, type, or date (with smooth scrolling)
- **🎨 Full customization** — Colors, borders, corner radius, font, icons, item counter
- **📂 Multi-desktop support** — Public desktop, OneDrive, and any additional folders
- **⏱️ Auto-archiving** — Move old files to an archive folder with configurable age
- **🌙 Zen Mode** — Hide/show all fences with double-click on desktop or `Ctrl+Alt+Z`
- **🌍 Multi-language** — English, Spanish, German, French, Portuguese, Italian
- **🪶 Ultralight** — ~780 KB binary, ~4 MB RAM idle, 0% CPU (fully event-driven, no polling)
- **🔔 Toast notifications** — Visual feedback with contextual icons (🟢 drops, 🔵 organization)
- **🤖 Local AI (Ollama)** — Semantic classification and automatic fence creation via local LLM

## 📥 Download

| Format | File | Best for |
|---|---|---|
| 🖥️ **Installer (EXE)** | `ZenDesktop-1.0.8-setup.exe` | Most users — wizard installer with Start Menu shortcut |
| 📦 **Installer (MSI)** | `ZenDesktop-v1.0.8-x64.msi` | Enterprises / system-wide installs with clean uninstall |
| 💾 **Portable** | `ZenDesktop-v1.0.8-portable.zip` | USB drives, custom paths, no installation |

All downloads are available at **[Releases](https://github.com/jaimitus/ZenDesktop/releases)**.

### 🖥️ EXE Installer (recommended)

1. Download `ZenDesktop-1.0.8-setup.exe` from [Releases](https://github.com/jaimitus/ZenDesktop/releases)
2. Run the installer — accepts the license, installs to `Program Files\ZenDesktop`
3. Start Menu shortcut is created automatically
4. Uninstall via Windows Settings → Apps, or re-run the installer

### 📦 MSI Installer

1. Download `ZenDesktop-v1.0.8-x64.msi` from [Releases](https://github.com/jaimitus/ZenDesktop/releases)
2. Run the installer — it installs to `Program Files\ZenDesktop` for all users
3. Start Menu shortcut is created automatically
4. Uninstall via Windows Settings → Apps, or re-run the MSI

### 💾 Portable (for USB drives / custom paths)

1. Download `ZenDesktop-v1.0.8-portable.zip` from [Releases](https://github.com/jaimitus/ZenDesktop/releases)
2. Extract to any folder (e.g. `%APPDATA%\ZenDesktop\`)
3. Run `ZenDesktop.exe` — it minimizes to the system tray
4. **Optional**: Add a shortcut to `shell:startup` for auto-start with Windows

> 💡 **Auto-update** works with the portable version too. When you click "Download & Install" in Settings → Updates, the new version replaces the current .exe and restarts automatically.

> 🔐 **Security**: Every update is cryptographically verified with an **Ed25519 signature** before it is installed. If the signature check fails, the download is discarded and the current version is left untouched. Released binaries also ship with a `SHA256SUMS.txt` so you can verify downloads manually.

> ⚠️ **Installed users (EXE/MSI)**: The app installs to `Program Files`, where standard users can't write. The in-app updater detects this and asks for administrator permission (UAC) automatically; you can also download the new installer from the release page instead.

### From source

```bash
git clone https://github.com/jaimitus/ZenDesktop.git
cd ZenDesktop
cargo build --release
# Binary at target/release/zendesktop.exe
```

## 🚀 Quick Start

| Action | Shortcut / Gesture |
|---|---|
| Open settings | Right-click tray icon → Settings |
| Zen Mode (toggle) | Double-click empty desktop or `Ctrl+Alt+Z` |
| Move files between fences | Select → Drag to another fence |
| Drop into subfolder | Drag over a folder inside a fence |
| Return to desktop | Drag outside all fences |
| Search in a fence | Click the search bar 🔍 |
| Change sort order | Right-click → Sort by |
| Lock fence position | Click the lock icon 🔒 |

## ⚙️ Configuration

Edit via the Settings window (tray icon) or directly in `config.toml`:

```toml
[general]
language = "en"
sweep_interval_minutes = 15

[appearance]
background = "#1A1B2E"
corner_radius = 12.0
font_family = "Segoe UI"

[[rules]]
id = "documents"
folder = "Documents"
extensions = ["pdf", "docx", "txt", "md"]
color = "#4CCD3C"
enabled = true
```

## 🏗️ Architecture

- **Rendering**: Direct2D + DirectWrite on *layered* windows (per-pixel alpha, 0% VRAM reserved)
- **File watching**: Native `ReadDirectoryChangesW` via `notify` — no polling, 0% CPU idle
- **Drag & Drop**: Manual capture (`SetCapture`) + `WM_MOUSEMOVE` for inter-fence drag; OLE `IDropTarget` for Explorer drop
- **I18n**: Static translation system with `Tr` struct — zero runtime allocations
- **Binary**: ~780 KB uncompressed (~420 KB with UPX), no external runtime

## 📁 Project Structure

```
├── Cargo.toml             # Dependencies & release profile
├── build.rs               # Windows resource compiler (.rc)
├── assets/
│   ├── zendesktop.rc      # EXE icon & metadata
│   └── icons/             # App & tray icons
├── src/
│   ├── main.rs            # Entry point & message loop
│   ├── config.rs          # config.toml load/save (serde)
│   ├── rules.rs           # Rule engine & organization
│   ├── ui.rs              # Windows, rendering, drag & drop
│   ├── settings.rs        # Settings window
│   ├── watcher.rs         # File system watcher
│   ├── ai.rs              # HTTP client for Ollama
│   ├── updater.rs         # Auto-update via GitHub Releases
│   └── i18n.rs            # Translations (6 languages)
├── scripts/
│   └── build-release.ps1  # Local release builder
├── installer/
│   └── zendesktop.wxs     # WiX v4 MSI installer source
└── .github/workflows/
    └── release.yml        # CI/CD release automation
```

## 🤝 Contributing

1. Fork the repository
2. Create a branch (`git checkout -b feature/name`)
3. Commit (`git commit -m "feat: description"`)
4. Push (`git push origin feature/name`)
5. Open a Pull Request

## 📄 License

MIT © 2024-2026 ZenDesktop Core Team

---

Built with ❤️ and Rust. [Report a bug](https://github.com/jaimitus/ZenDesktop/issues) · [Discussions](https://github.com/jaimitus/ZenDesktop/discussions)
