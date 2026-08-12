# Changelog

All notable changes to ZenDesktop are documented here.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and versioning follows [SemVer](https://semver.org/).

---

## [1.0.0] - 2026-08-12

### 🎉 First stable release

After months of development, ZenDesktop 1.0.0 is production-ready.

### ✨ Features

- **Floating translucent fences** with Direct2D + DirectWrite rendering
- **Manual Drag & Drop** between fences, subfolders, and to the desktop
- **Visual highlight** when dragging over target subfolders
- **Explorer drop** via OLE `IDropTarget` (RegisterDragDrop)
- **Auto-organization** by rules (extension, name, pattern)
- **AI classification** via Ollama (local)
- **Zen Mode** with double-click on desktop or `Ctrl+Alt+Z`
- **6 languages**: English, Spanish, German, French, Portuguese, Italian
- **Toast notifications** with contextual icon (🟢 green for drops, 🔵 blue for organize)
- **Integrated search** in each fence
- **Grid and list** view modes
- **Sorting** by name, size, type, date, or custom
- **Customizable themes**: colors, borders, typography, icons
- **Auto-archiving** by file age
- **Multi-desktop support** (OneDrive, Public, etc.)
- **System tray** with context menu
- **Ultralight binary**: ~780 KB, ~4 MB RAM idle
- **Auto-update** via GitHub Releases API
- **6-language i18n** with static translation system

### 🔧 Improvements

- MSI installer via WiX v4 with Start Menu shortcuts
- Toast uses GDI `DrawTextW` for full Unicode support
- Toast width dynamically measured with `GetTextExtentPoint32W`
- Green checkmark icon on drop toasts
- Blue icon for organization toasts
- Settings → Updates panel with check/download buttons
- Auto-restart after update (portable + installed)
- Dead code removed (COM drag fallback, bitmap font, unused imports)
- Zero compiler warnings

### 🐛 Fixes

- Drag crash fixed (manual capture without COM)
- Desktop drops no longer auto-reorganized
- Empty toast fixed (now uses GDI DrawTextW)
- Cursor changes to hand + file icon during drag
- Auto-organizer no longer reverts manual desktop drops
- Subfolder detection during drag-and-drop

---

[1.0.0]: https://github.com/jaimitus/ZenDesktop/releases/tag/v1.0.0
