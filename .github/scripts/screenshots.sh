#!/usr/bin/env bash
# Real screenshots of the built app on the macOS runner, with demo data (BOARD_DEMO=1: three made-up cards, no network
# for them). Usage: screenshots.sh <Board.app> <out dir>
set -uo pipefail
src="$1"; out="$2"; mkdir -p "$out"
app=/Applications/Board.app
rm -rf "$app"; cp -R "$src" "$app"; xattr -cr "$app"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$app"
shot() { screencapture -x "$out/$1.png"; echo "shot $1"; }
launch() { open -n -a "$app" --env BOARD_DEMO=1 --args --demo; }
quit() { osascript -e 'tell application "Board" to quit' 2>/dev/null; sleep 2; pkill -x Board 2>/dev/null; sleep 1; }

# Click a button by name anywhere in Notification Center (the permission prompt, then a notification's actions).
click_nc() {
  osascript <<EOF 2>&1
tell application "System Events"
  tell process "NotificationCenter"
    set found to false
    repeat with w in windows
      try
        repeat with e in (entire contents of w)
          try
            if (role of e is "AXButton") and ((name of e is "$1") or (description of e is "$1")) then
              click e
              set found to true
              exit repeat
            end if
          end try
        end repeat
      end try
      if found then exit repeat
    end repeat
    return found
  end tell
end tell
EOF
}
dump_nc() {
  osascript -e 'tell application "System Events" to tell process "NotificationCenter" to get {role, name, description} of entire contents of windows' > "$out/nc-tree-$1.txt" 2>&1 || true
}

defaults write com.apple.ncprefs dnd_prefs -data 0 2>/dev/null || true
launch; sleep 8
shot 01-first-launch
dump_nc 1
echo "allow: $(click_nc Allow)"
sleep 2; shot 02-after-allow
quit

# Second launch: permission granted, the demo cards arrive as notifications and the menu-bar count shows.
launch; sleep 6
shot 03-notifications
dump_nc 3
# Menu-bar dropdown: Board's status item is in its process's second menu bar.
osascript -e 'tell application "System Events" to tell process "Board" to click menu bar item 1 of menu bar 2' 2>&1 | head -2
sleep 1.5; shot 04-menu-bar
osascript -e 'tell application "System Events" to key code 125' -e 'delay 0.2' -e 'tell application "System Events" to key code 125' -e 'delay 0.2' -e 'tell application "System Events" to key code 124' 2>&1 | head -2
sleep 1; shot 05-menu-answer
osascript -e 'tell application "System Events" to key code 53' -e 'delay 0.2' -e 'tell application "System Events" to key code 53' 2>/dev/null
sleep 1
# Quick add, opened the way Shortcuts or a link would.
open "board://add?text=Book%20the%20dentist%20for%20next%20week"
sleep 2; shot 06-quick-add
osascript -e 'tell application "System Events" to key code 53' 2>/dev/null
# The main window (the real Board page; on a runner it shows the Cloudflare Access sign-in).
osascript -e 'tell application "System Events" to tell process "Board" to set frontmost to true' 2>/dev/null
open "board://needs"; sleep 5; shot 07-window
quit
ls -la "$out"
