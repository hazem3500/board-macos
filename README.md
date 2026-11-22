# Board for macOS

The oz-home Board as a Mac app: the same Board page in a native window, plus a menu-bar count you can answer from,
notifications with answer buttons and inline reply, a Dock badge, ⌥⌘B Quick add from anywhere, a Work mode switch,
`board://` links, Services ("Add to Board", "Open in Board") and automatic updates. The Board itself stays behind
Cloudflare Access.

## Install

1. Download **[Board.dmg](https://github.com/hazem3500/board-macos/releases/latest/download/Board.dmg)** and drag
   Board to Applications.
2. First open only (the app isn't notarized): open Board, close the warning, then **System Settings → Privacy &
   Security → Open Anyway**. Or in Terminal: `xattr -dr com.apple.quarantine /Applications/Board.app`.
3. Sign in to the Board in the window (Cloudflare Access, as in the browser), then press **Connect** on the
   "Connect the Board app" card and allow notifications. The Mac then shows in the Board's Devices list, where it
   can be revoked.

Updates download in the background; "Restart to Update" appears in the menu-bar menu. After an update macOS may ask
once to let Board use its Keychain item: choose Always Allow.

## Links (Shortcuts, Raycast, Alfred)

`board://needs`, `board://board`, `board://projects`, `board://card/<id>`, `board://add?text=…` (opens Quick add
prefilled), `board://open?url=https://board.zem.cool/…`.

## This repository

Builds and releases only. The app's source lives in a private repository; each release tag there pushes a snapshot
to `app/` and tags `vX.Y.Z` here, and `.github/workflows/build.yml` builds it on GitHub's macOS runners and publishes
the release (`Board.dmg`, the signed update bundle and `latest.json`, which the app's updater reads). Branch pushes
build and take screenshots with demo data.
