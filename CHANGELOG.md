# Changelog

All notable changes to ZenDesktop are documented here.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and versioning follows [SemVer](https://semver.org/).

---

## [1.0.21] - 2026-08-14

### Added
- **🖼️ Real thumbnails in grid mode** — image files show a real thumbnail
  (WIC-based, shared LRU cache) inside grid cells instead of a generic icon,
  for a much nicer browsing experience with photo folders.
- **📊 Per-fence statistics tooltip** — hovering a fence title shows a
  multi-line tooltip with the item count, total size and file-type breakdown.
- **📊 System Monitor widget** — a first-class widget box (CPU / RAM /
  battery bars, refreshed every second) managed from its own Settings tab
  (enable/disable, update interval), just like the Spotify and Dropbox
  widgets.
- **🤖 AI-suggested rules** — a "Suggest desktop rules" button in Settings →
  Rules asks your local Ollama model for a rule proposal and lets you review
  it with checkboxes before adding only the rules you tick (additive — never
  replaces your existing rules).
- **🔄 Automatic config backup** — config is backed up silently before each
  save (throttled to 1/hour, keeping the 12 most recent copies) with a
  readable timestamp in the filename, plus an "Open backups folder" button in
  Settings → General.

## [1.0.20] - 2026-08-14

### Added
- **🪟 Background material (Mica / Acrylic / Blur)** — a new **Background
  Material** section in Settings → Appearance picks the box backdrop:
  **None**, **Acrylic**, **Blur** or **Mica**, with a background opacity slider
  and an acrylic tint setting. Boxes become Windows-11-style translucent
  surfaces (acrylic/blur use `SetWindowCompositionAttribute`, Mica uses the
  DWM backdrop).
- **🪄 Real rounded corners** — boxes are clipped with a rounded window region
  that is re-synced on every frame (create, resize, roll-up, live preview), so
  the corners stay rounded with the wallpaper showing through even when a
  material is active. Includes `DWMWA_WINDOW_CORNER_PREFERENCE` so DWM also
  rounds the window, and exact 1:1 geometry (no more 1px sliver where the
  squared backdrop leaked through).
- **🎧 Spotify empty state** — when Spotify is connected but nothing is
  playing, the widget now shows the **embedded Spotify logo** with
  “Nothing playing”, a hint and an **Open Spotify** button (launches the
  desktop app) instead of a dead player with blank art and zeroed bars.
- **🎨 Embedded Spotify logo** — `logo_spotify.png` ships inside the binary:
  auto-cropped to the glyph, white background converted to transparency and
  downscaled to 128px (crisp even at 150% DPI).
- **⏩ Spotify seek** — click or drag the progress bar to jump to any
  position in the current track.

### Fixed
- **Invisible D2D primitives** — `FillEllipse`/`DrawEllipse` don't render
  anything on this pipeline (GDI-compatible DC render target); every ellipse
  was silently invisible. All five call sites now draw circles with
  `FillRoundedRectangle`/`DrawRoundedRectangle` (a square with radius = half
  its side is an exact circle): the **volume slider knob**, the **pin heads**
  (📌 header pin and item favorites), the **lock keyhole eye**, and the Lua
  `circle` / `circle_stroke` primitives — previously these rendered nothing.

---

## [1.0.19] - 2026-08-14

### Added
- **📁 Dropbox widget (first-class)**: a new **📁 Dropbox** panel in Settings
  manages a desktop box that browses and syncs your Dropbox, exactly like the
  Spotify widget:
  - **App Key / App Secret / Redirect URI** — editable fields persisted in
    `config.toml`, with a live session status and **Connect / Disconnect**
    buttons. OAuth uses PKCE with `force_reapprove` so re-authorizing picks
    up newly enabled scopes (fixes 401 `missing_scope` on stale tokens).
  - **Enable the widget on the desktop** — creates/removes the box instantly.
  - **File browser in the box**: click to select, double-click a folder to
    enter it (breadcrumb shows the remote path), ⬆ to go up, double-click a
    file to download and open it, and a **Sync** button for local ↔ remote.
  - **Bidirectional drag & drop**: drag files out of the box (downloaded to
    a temp file, then a native OLE drag) or drop files into the box from
    Explorer or another fence (uploaded to the remote folder you're
    browsing).

### Fixed
- **Spotify controls and volume did nothing**: the play/pause, next/previous
  and volume calls are PUT/POST without a body, and ureq 2.x doesn't send
  `Content-Length` for bodyless requests — Spotify rejected them with
  `411 Length Required`. All four control calls now send `Content-Length: 0`.
- **Spotify volume always showed 50%**: the now-playing endpoint doesn't
  include the device, so the slider fell back to a hardcoded 50. The widget
  now queries the active device (real volume + device name).
- **Crash (access violation) when dropping onto boxes**: the OLE callbacks
  built the `IDataObject` with `&*(pdataobj as *const IDataObject)`, which
  read the COM object's vtable as if it were the pointer, so any COM call
  jumped to `QueryInterface + offset`. The interface is now constructed with
  `IDataObject::from_raw`, and internal drags skip COM inspection entirely.
- **Heap corruption (0xc0000374) when dropping into the Dropbox box**: the
  drop callback no longer touches the UI or re-reads the `IDataObject` in
  the middle of a `DoDragDrop`. Drops are queued and processed by a message
  after the drag loop ends; internal drops reuse the source fence's paths.

## [1.0.18] - 2026-08-13

### Added
- **Spotify widget settings tab**: new **🎧 Spotify** panel in Settings with
  an enable/disable switch (creates/removes the box instantly), editable
  Client ID, Client Secret and **Redirect URI**, live session status, and
  **Connect with Spotify** / **Disconnect** buttons — no more hand-editing
  `config.toml` or the code. The Redirect URI is editable so it can match
  the one registered on `developer.spotify.com` exactly (default
  `http://127.0.0.1:8899/callback`), and the local auth listener binds to its
  port.
- **First-class Spotify widget**: the now-playing box (cover art, progress
  with elapsed/total times, volume slider, prev/play-pause/next controls,
  device name, queue with jump-to-track) is now a first-class desktop box
  created automatically at startup when enabled, instead of a hidden
  config-only feature.

### Changed
- Spotify credentials are **no longer hardcoded** — Client ID / Client
  Secret start empty and are configured from Settings (nothing ships
  compiled-in).
- **Widget boxes keep their position**: `Config::normalize()` no longer
  drops fences whose id isn't a rule, so widget boxes (e.g. `widget:clima`)
  survive settings previews and config saves.

### Fixed
- **Spotify box is draggable**: clicks in the box body now only capture the
  widget controls; header drags move the box and the corner grip resizes it
  like any other fence (previously every click was swallowed).

## [1.0.17] - 2026-08-13

### Added
- **Lua widget framework**: boxes that run user scripts from the widgets
  folder (`widgets/` next to the exe, or `%APPDATA%\ZenDesktop\widgets\`).
  Manage them in Settings → 🧩 Widgets: create, edit the code inline, rename,
  delete, disable, or instantiate a box with "Añadir como caja" (no form — it
  appears on the desktop immediately and is dragged/resized like any fence).
- **Bundled example widgets** auto-installed on first run: `Reloj` (clock),
  `Notas` (todo list), `Clima` (Open-Meteo, no API key) and `Contador`
  (interactive counter demo). They install only when the widgets folder is
  empty or contains only bundled examples, so user scripts are never touched.
- **Expanded sandbox API** so widgets can be truly complex:
  - Drawing: `line`, `circle`, `circle_stroke`, `round_rect` (with border),
    `text_center` / `text_right` (real measured sizes via DirectWrite);
  - Images: `ctx:image(url, x, y, w, h)` — async download + bounded cache
    (32 entries, 1024 px max, 5 min TTL), JPEG/PNG/GIF/BMP;
  - Interactivity: `function click(x, y, w, h)` is called on body presses
    (header drags and the resize grip still work normally);
  - Persistent state: the global `state` table survives across renders and
    clicks;
  - System API: `app:open(path)` (launch with default app), `app:notify(msg)`
    (toast), `app:version()`;
  - HTTP API: `http:get` / `http:get_json` (async, cached, never blocks UI).
- **Spotify widget**: an OAuth PKCE now-playing box with cover art, progress
  bar with elapsed/total times, volume slider (drag to adjust), previous /
  play-pause / next controls, playback device name, disconnect button, and a
  queue view (☰) where clicking any track jumps to it (with mouse-wheel
  scroll). The Spotify Client ID lives in the code (`SPOTIFY_CLIENT_ID`),
  no configuration needed.
- **AI rule generation**: describe a rule in plain text and Ollama creates it
  (title, folder, extensions, name patterns, regex); the AI can also
  auto-cluster the desktop into suggested rules using embeddings.
- **Undo moves**: files moved into fences can be undone (undo stack with
  retry when the destination is locked).
- **Per-fence list/grid view**: each rule picks its own view mode
  (Auto/List/Grid) from Settings → Rules, overriding the global appearance.

### Fixed
- **"Añadir como caja" worked but the box vanished instantly**: the live
  settings preview calls `normalize()`, which pruned every fence whose id was
  not a rule — widget boxes (`widget:clima`) were deleted a split second
  after being created. `normalize()` now keeps widget fences.
- **Widget boxes could not be moved/resized**: body-click interception was
  swallowing header and resize-grip presses too. Only body clicks are
  intercepted now; header drags move the box and the corner grip resizes it
  like any other fence. Invisible lock/pin hit-tests are disabled on widget
  boxes.
- **Contador Reset button**: `click` now receives the real body size
  (`x, y, w, h`), so hit-testing stays correct when the box is resized.
- **Spotify Client ID field** removed from the Settings UI — the ID is
  compiled in; configs that already have one keep it.

## [1.0.16] - 2026-08-13

### Added
- **File favorites (pin to top)**: pin important files so they always float
  above a fence, regardless of the sort order. Toggle with `Ctrl+P` on the
  selected item or by clicking the pin button that appears on hover; pinned
  files show a pin badge in list and grid view and persist per rule (including
  virtual and tabbed fences).
- **Dedicated language tab**: Settings now has its own 🌐 Language panel with
  flag cards, instead of crowding the General tab (which also fixes the overlap
  with the export/import configuration section).

### Fixed
- **Settings scroll residue**: scrolling the settings panels no longer leaves
  black lines over the input fields. Native edit controls are now repositioned
  and shown/hidden outside the Direct2D draw cycle (before/after `EndDraw`),
  so their previous positions are always repainted.

## [1.0.15] - 2026-08-13

### Added
- **Keyboard navigation**: arrow keys move the selection (Home/End jump to
  first/last, Shift extends the selection). Works in both list and grid
  view, auto-scrolls to keep the selected item visible, and pairs with
  Enter (open) and Delete (trash).
- **Advanced rules**: rules can now filter by file size (min/max, human
  readable like `5 MB`), age (newer/older than N days), and a full regular
  expression on the file name. Filters combine with the existing
  extension/name patterns and apply to virtual fences and auto-organization.

### Fixed
- **F2 rename now works**: fences are non-activating windows, so clicking a
  file never gave the fence keyboard focus and F2/Delete/Enter keystrokes
  went to the previously focused app. Clicking an item now activates and
  focuses the fence, so F2 rename, Delete, Enter, Ctrl+A and arrow keys all
  work right after selecting a file.

## [1.0.14] - 2026-08-13

### Added
- **Pin to Top (always-on-top)**: a pin button next to the lock in each fence
  header makes that fence float above any app (`HWND_TOPMOST`), ideal for an
  Inbox or a notes box. The state persists with the layout, survives layout
  templates, and can also be toggled from the right-click menu.
- **Per-fence icon size**: each rule can override the global grid icon size
  with a cycling chip in Settings → Rules (Auto → 16 → 24 → 32 → 48 → 64 →
  96 px). `Auto` inherits the global Appearance setting.
- **Header tooltips**: hovering the lock or pin icon now shows a floating
  tooltip above the fence explaining the action, localized in all six
  languages.

## [1.0.13] - 2026-08-13

### Changed
- **Live settings preview**: options now apply instantly as you change them
  (theme, colors, sizes, rules, language, folders…), so you see the result
  immediately without an Apply button.
- Removed the **Apply** button: **Save** persists the changes and closes,
  **Cancel** reverts the live preview back to the state before opening
  Settings. Text fields commit when they lose focus or on Enter.
- An **unsaved changes** indicator (accent dot + label) appears in the
  Settings bar while the preview differs from the saved state.
- **Debounced preview**: rapid changes (typing + blur, fast toggles) coalesce
  into a single fence rebuild ~200 ms after the last change, so the desktop
  doesn't flicker while editing.
- **State preserved on rebuild**: fences keep their scroll position, selection,
  search text and active tab across live-preview rebuilds, so changing a
  color no longer resets what you were looking at.
- **Selective organize**: saving now only runs an organization pass when the
  rules or folders actually changed, so a purely visual tweak no longer
  sweeps the desktop.

### Fixed
- Settings sidebar now shows the real build version instead of a hardcoded
  `v1.0.11`.

## [1.0.12] - 2026-08-13

### Added
- **Layout templates**: save named snapshots of all fence positions from
  Settings → General and reapply them with one click (clickable chips).
  Each template can be marked as the default (star icon).
- **Multi-monitor aware templates**: templates remember which monitor each
  fence lives on; applying one repositions fences onto their monitors even
  when the display setup changed (moved, swapped, resized, or unplugged
  monitors) — and never leaves a fence off-screen.
- **Auto-restore on known monitors**: when the current monitor arrangement
  matches the default template (e.g. reconnecting a dock), the layout
  restores itself automatically — at startup if the display changed since
  the last session, and live whenever monitors are connected/disconnected.
- **Configurable startup delay**: fences appear after a configurable delay
  (0–600 s) so they don't cover the desktop while Windows finishes booting.
- **Toast queue**: notifications now queue up (max 5) instead of overwriting
  each other, and clicking one dismisses it instantly.
- **Magnetic snap guides**: blue alignment guides while dragging fences —
  edges, centers, and same-size alignment with other fences and the monitor.

### Changed
- **Pretty empty state**: empty fences show a centered icon and a hint
  ("Drop files here") instead of plain text.

## [1.0.11] - 2026-08-12

### Changed
- Test release to verify the end-to-end automatic update flow (1.0.10 ->
  1.0.11) over the CDN-only updater: detection, download with retries,
  signature verification, UAC elevation, mutex handoff and restart.

## [1.0.10] - 2026-08-12

### Fixed
- **Update checks no longer hit the GitHub API**: the public API rate-limits
  per IP (60/hour), which made the updater report "status code 403" and
  blocked updates entirely. The latest version is now read from a small
  `version.txt` published on the downloads CDN (no rate limit), and the
  exe/signature URLs are built by convention.

## [1.0.9] - 2026-08-12

### Changed
- Test release to verify the end-to-end automatic update flow from an
  installed build (1.0.8 -> 1.0.9: detection, download with retries,
  signature verification, UAC elevation, mutex handoff and restart).

## [1.0.8] - 2026-08-12

### Fixed
- **Robust update downloads**: GitHub's CDN intermittently cuts connections
  mid-transfer ("Network Error: Unexpected EOF"). The updater now retries
  with backoff, and signature verification happens inside the retry loop, so
  a truncated download is discarded and re-downloaded instead of failing.
- **Better UAC diagnostics**: if elevation is cancelled or blocked, the
  message now includes the real Windows error code (1223 = cancelled by
  user, 5 = denied/blocked) so the cause is identifiable.

## [1.0.7] - 2026-08-12

### Changed
- Test release to verify the end-to-end automatic update flow from an
  installed build (1.0.6 -> 1.0.7, including the UAC elevation path).

## [1.0.6] - 2026-08-12

### Changed
- **Self-update works for installed builds**: downloads are staged in a
  writable location and, when the app is installed under `Program Files`,
  the updater asks for administrator permission (UAC) automatically to
  replace the executable. Portable installs update without elevation.

## [1.0.5] - 2026-08-12

### Changed
- Release to verify the end-to-end automatic update flow (1.0.4 -> 1.0.5).

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

[1.0.6]: https://github.com/jaimitus/ZenDesktop/releases/tag/v1.0.6
[1.0.5]: https://github.com/jaimitus/ZenDesktop/releases/tag/v1.0.5
[1.0.4]: https://github.com/jaimitus/ZenDesktop/releases/tag/v1.0.4
[1.0.3]: https://github.com/jaimitus/ZenDesktop/releases/tag/v1.0.3
[1.0.2]: https://github.com/jaimitus/ZenDesktop/releases/tag/v1.0.2
[1.0.1]: https://github.com/jaimitus/ZenDesktop/releases/tag/v1.0.1
[1.0.0]: https://github.com/jaimitus/ZenDesktop/releases/tag/v1.0.0
