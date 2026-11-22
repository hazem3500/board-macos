#!/usr/bin/env bash
# Real screenshots of the built app on the macOS runner, with demo data (BOARD_DEMO=1: three made-up cards, no
# network for them). Usage: screenshots.sh <Board.app> <out dir>
set -uo pipefail
src="$1"; out="$2"; mkdir -p "$out"
app=/Applications/Board.app
bundle=cool.zem.board.mac
rm -rf "$app"; cp -R "$src" "$app"; xattr -cr "$app"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$app"
brew install -q cliclick >/dev/null 2>&1 || true
shot() { screencapture -x "$out/$1.png"; echo "shot $1"; }
launch() { open -n -a "$app" --env BOARD_DEMO=1; }
quit() { osascript -e 'tell application "Board" to quit' 2>/dev/null; sleep 2; pkill -x Board 2>/dev/null; sleep 1; }
ncprefs() { plutil -convert xml1 -o - ~/Library/Preferences/com.apple.ncprefs.plist 2>/dev/null | grep -A12 "$bundle" | head -30; }

launch; sleep 8
shot 01-first-launch
echo "--- ncprefs entry after the permission request:"; ncprefs
log show --last 2m --style compact --predicate 'process == "usernoted" OR subsystem == "com.apple.UserNotifications"' 2>/dev/null | grep -i "board\|$bundle" | tail -15
quit

# The runner has nobody to click "Allow": grant it the way System Settings would (alerts, sounds, badges on) and
# restart the notification daemons.
/usr/bin/python3 - "$bundle" <<'PY'
import plistlib, os, sys, subprocess
bundle = sys.argv[1]
path = os.path.expanduser('~/Library/Preferences/com.apple.ncprefs.plist')
subprocess.run(['defaults', 'read', 'com.apple.ncprefs'], capture_output=True)
data = plistlib.load(open(path, 'rb')) if os.path.exists(path) else {}
apps = data.setdefault('apps', [])
entry = next((a for a in apps if a.get('bundle-id') == bundle), None)
if entry is None:
    entry = {'bundle-id': bundle, 'path': '/Applications/Board.app', 'content_visibility': 0, 'grouping': 0, 'src': []}
    apps.append(entry)
print('before', {k: entry.get(k) for k in ('flags', 'auth')})
entry['auth'] = 7
entry['flags'] = (entry.get('flags', 0) & ~0x38) | 0x10  # alert style: Alerts (stay until dismissed)
plistlib.dump(data, open(path, 'wb'), fmt=plistlib.FMT_BINARY)
print('after', {k: entry.get(k) for k in ('flags', 'auth')})
PY
echo '--- Focus state:'; ls ~/Library/DoNotDisturb/DB/ 2>&1; for f in Assertions ModeConfigurations; do plutil -p ~/Library/DoNotDisturb/DB/$f.json 2>/dev/null | head -40; done
# Screen sharing (how the runner is watched) turns on "Do Not Disturb while mirroring": switch that off.
defaults write com.apple.ncprefs dnd_mirroring -int 0 2>/dev/null
defaults -currentHost write com.apple.notificationcenterui doNotDisturb -bool false 2>/dev/null
killall cfprefsd usernoted NotificationCenter donotdisturbd 2>/dev/null; sleep 3

# Second launch: the demo cards arrive as notifications and the menu-bar count shows.
launch; sleep 5
shot 02-notifications
sleep 2; shot 03-notifications-later
# Menu-bar dropdown: click Board's status item where macOS put it.
pos=$(osascript -e 'tell application "System Events" to tell process "Board" to get {position, size} of menu bar item 1 of menu bar 2' 2>&1)
echo "status item: $pos"
read -r x y w h <<<"$(echo "$pos" | tr -d ' ' | tr ',' ' ')"
if [[ -n "${h:-}" ]]; then cliclick "c:$((x + w / 2)),$((y + h / 2))"; fi
sleep 1.5; shot 04-menu-bar
# Into the first card's submenu (its answers).
cliclick "kp:arrow-down" "kp:arrow-right" 2>&1 | head -2
sleep 1; shot 05-menu-answer
cliclick "kp:esc" "kp:esc" "kp:esc" 2>/dev/null; sleep 1
# Quick add, opened the way Shortcuts or a link would.
open "board://add?text=Book%20the%20dentist%20for%20next%20week"
sleep 2; shot 06-quick-add
cliclick "kp:esc" 2>/dev/null
# Notification Center with the cards (click the clock).
log show --last 1m --style compact --predicate 'process == "usernoted"' 2>/dev/null | grep -iE "present|focus|suppress|dnd" | tail -8
cliclick "c:960,11" 2>/dev/null
sleep 2; shot 07-notification-center
cliclick "m:880,120" 2>/dev/null; sleep 1; shot 08-notification-hover
cliclick "kp:esc" 2>/dev/null
quit
ls -la "$out"
