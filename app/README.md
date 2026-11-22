# Board for macOS

A Tauri v2 app around the same Board page (`https://board.zem.cool/oz/`, no UI fork), with the native parts a Mac
should have. Install: the latest `Board.dmg` from https://github.com/hazem3500/board-macos/releases/latest.

## What it does

| Feature | How |
|---|---|
| Main window | WKWebView on `/oz/`; Cloudflare Access session kept in WebKit's persistent store; transparent title bar tinted to the page; closing hides it (the app stays in the menu bar) |
| Menu bar | Count of Needs me next to the Board glyph; dropdown lists the top 8 cards; each opens a submenu with its options (recommended first), Reply…, Open in Board; Work mode switch with its schedule label; Quick Add; Open at Login; updates |
| Notifications | Local UserNotifications from a 30 s signed read while the app runs: one category per choice card (its options as actions), inline Reply for text questions, Open for plans/Draw; taken back when the card is answered anywhere |
| Dock badge | The same count |
| Quick add | ⌥⌘B anywhere (global shortcut) opens a floating panel; Return adds to `personal/inbox` through the signed-in page |
| Links | `board://needs`, `board://board`, `board://projects`, `board://card/<id>`, `board://add?text=…` (Shortcuts "Open URL" works as "Add to Board"), `board://open?url=https://board.zem.cool/…` |
| Services | "Add to Board" for selected text; "Open in Board" for a selected Board link |
| App menu | Board, File (New Task ⌘N, Quick Add ⌥⌘B), Edit (with Find ⌘F, Search or Jump To ⌘K), View (Needs Me ⌘1, Board ⌘2, Priorities ⌘3, Activity ⌘4, Reload, zoom, full screen), Window, Help |
| Updates | tauri-plugin-updater reads `latest.json` from the latest release every 6 h, installs in the background, "Restart to Update" in the menu bar |

Reads, answers and Work mode use the signed device routes (`/api/device/*`), the same design and server code as the
Android app: the Board page hands the app a one-time code and the device-tier Access token (`pwa.js` `pairAppUrl`,
caught inside the app's own web view), the app makes a P-256 key and keeps the credential in the login Keychain. The
Mac shows in the Board's Devices list and can be revoked there. Security packet:
`/home/hazem/OpenBot/Shared/status/board-macos-20261122/SECURITY-PACKET.md`.

## First open (not notarized)

Board is ad-hoc signed, not notarized (no paid Apple Developer account). The first time: open Board, dismiss the
warning, then System Settings → Privacy & Security → **Open Anyway**. Or in Terminal:
`xattr -dr com.apple.quarantine /Applications/Board.app`. Updates install in place without the prompt; macOS may ask
once per update to let Board read its Keychain item ("Always Allow").

## Build and release

- PRs: `.github/workflows/macos.yml` runs the unit tests (`cargo test --lib`) on Linux.
- Mac builds run free on GitHub's macOS runners in the public `hazem3500/board-macos`: a `macos-vX.Y.Z` tag on a main
  commit here pushes a snapshot of this folder to its `app/` and tags `vX.Y.Z`; its workflow builds a universal app,
  signs the update bundle (minisign key in its tag-only `release` environment) and publishes `Board.dmg`,
  `Board.app.tar.gz`(+`.sig`), `latest.json` and checksums. Branch pushes there build and take screenshots with demo
  data (`BOARD_DEMO=1`).
- Icons: `icon-src/macos.svg` → `src-tauri/icons/app-icon.png` (1024) and `icon-src/tray.svg` → `tray.png` (44, a
  template image); CI generates the `.icns` with `tauri icon`.
