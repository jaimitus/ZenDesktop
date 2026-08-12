# Changelog

All notable changes to ZenDesktop are documented here.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and versioning follows [SemVer](https://semver.org/).

---

## [1.0.4] - 2026-08-12

### Fixed
- **Update detection works now**: the version comparison in the update
  check was inverted, so the app always reported "you are up to date" even
  when a newer release existed. It now correctly offers the new version.

## [1.0.3] - 2026-08-12

### Fixed
- **Auto-update now really applies**: after downloading and verifying the new
  build, ZenDesktop closes the old instance, waits for it to release the
  single-instance mutex, and hands over to the new version — no more
  "update installed" that never restarts.
- **Upgrade path from v1.0.2**: builds already running the previous (broken)
  updater can still update, since the new binary detects the leftover backup
  and takes over the running instance.

## [1.0.2] - 2026-08-12

### Added
- **Tab grouping controls**: link/unlink rules into tabs from Settings, and name each group with a title shown in the fence header.
- **Per-rule view mode**: choose List or Grid per rule, with a configurable icon size for grid cells.
- **Sharp icons at high DPI**: large and grid icons now come from the system image list (EXTRALARGE/JUMBO), staying crisp at 150%+ scaling.

### Changed
- **Bounded icon cache**: the icon cache is now a real LRU with per-size memory caps for paths and extensions.

### Fixed
- **Default folders**: ZenDesktop and ZenArchive now default to the Documents folder instead of the desktop.
- **Settings layout**: languages no longer overlap export/import; the Updates tab was tidied.
- **Toasts**: added padding so text is no longer clipped.

---

## [1.0.1] - 2026-08-12

### Added
- **Tabbed Fences**: Group multiple fences or rules under tabs within a single floating window. Navigate between tabs with a single click.
- **Roll-up Fences**: Double click on the title bar of any fence to instantly roll it up, leaving only the title visible to save desktop space.
- **Smooth Scrolling**: Mouse wheel scrolling on fences with many icons now uses smooth inertia-based interpolation instead of chunky steps.
- **F2 Rename**: Press `F2` on a selected item to directly rename it in place.

### Fixed
- **Drag & Drop Flicker**: Drastically reduced flickering when dragging items in and out of fences by updating the UI in-place instead of rebuilding it.
- **Drag & Drop Crash**: Fixed a crash (`0xc0000005` access violation) when dragging items out to Explorer that occurred during the OLE modal loop.
- **Animation Glitches**: Fixed several issues with hover animations getting stuck or snapping instantly to the end state, including the `TrackMouseEvent` bugs and Direct2D visual clipping conflicts.

---

## [1.0.0] - 2026-08-12

### Initial Release

ZenDesktop 1.0.0 is the first stable release — a lightweight, native Windows
desktop organizer built from scratch in Rust.

### Features

- **Floating translucent fences** — Direct2D + DirectWrite rendering with per-pixel alpha
- **Drag & Drop** — Move files between fences, into subfolders, or back to the desktop with visual feedback
- **Explorer integration** — Drop files from any folder directly into fences via native OLE `IDropTarget`
- **Auto-organization** — Rule-based classification by extension, name, or pattern with optional local AI
- **AI classification** — Semantic file organization using Ollama (local LLM, fully offline)
- **Zen Mode** — Instant hide/show all fences with `Ctrl+Alt+Z` or double-click on desktop
- **Multi-language** — 6 languages: English, Spanish, German, French, Portuguese, Italian
- **Toast notifications** — Contextual feedback with color-coded icons (green for drops, blue for organization)
- **Integrated search** — Real-time file filtering within each fence
- **Grid & List views** — Sortable by name, size, type, date, or custom order
- **Full customization** — Colors, borders, corner radius, fonts, icons, item counter
- **Auto-archiving** — Move stale files to an archive folder by configurable age
- **Multi-desktop** — Public desktop, OneDrive, and custom folder support
- **System tray** — Minimizes to tray with full context menu
- **Ultra-lightweight** — ~780 KB binary, ~4 MB RAM idle, 0% CPU (event-driven, zero polling)
- **Auto-update** — One-click updates from GitHub Releases with Ed25519 signature verification
- **MSI installer** — System-wide installation with Start Menu shortcuts via WiX v4

### Technical Highlights

- Pure Win32: Direct2D, DirectWrite, layered windows — no Electron, no browser runtime
- Native file watching via `ReadDirectoryChangesW` — zero CPU when idle
- Manual drag & drop with `SetCapture` + `WM_MOUSEMOVE` for inter-fence operations
- OLE drag target (`IDropTarget`) for accepting drops from Explorer and other apps
- Static i18n system with zero runtime allocations across 6 languages
- Ed25519 cryptographic signature verification for secure auto-updates
- Zero compiler warnings in release profile

### Known Limitations

- AI features require [Ollama](https://ollama.com) running locally
- SmartScreen may show a warning on portable builds (MSI installer is unaffected)
- Works on Windows 10 and 11 only

---

[1.0.4]: https://github.com/jaimitus/ZenDesktop/releases/tag/v1.0.4
[1.0.3]: https://github.com/jaimitus/ZenDesktop/releases/tag/v1.0.3
[1.0.2]: https://github.com/jaimitus/ZenDesktop/releases/tag/v1.0.2
[1.0.1]: https://github.com/jaimitus/ZenDesktop/releases/tag/v1.0.1
[1.0.0]: https://github.com/jaimitus/ZenDesktop/releases/tag/v1.0.0
