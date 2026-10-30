#!/bin/bash
# Set up phone access to Threshold on this Mac, over Tailscale.
#
#   scripts/threshold-mobile-setup.sh            set up, or change this Mac's name/colour
#   scripts/threshold-mobile-setup.sh --off      stop serving to phones
#
# Run it once on every Mac that should be reachable from your phone. It is
# safe to re-run: the machine keeps its id, and paired phones stay paired
# unless you rename the machine (a new name is a new address, so the phone
# needs pairing again).
#
# What it does:
#   1. Installs the Tailscale app if it is missing (Homebrew cask), and waits
#      for you to log in.
#   2. Asks what to call this Mac. That one name becomes its tailnet host
#      name, so its address is https://<name>.<tailnet>.ts.net, and it is
#      the name your phone shows on every screen.
#   3. Asks for a colour, shown as a band across every phone screen, so two
#      Macs can never be confused at a glance.
#   4. Puts Threshold on the tailnet with `tailscale serve` (HTTPS, tailnet
#      only, never the public internet). Threshold itself listens on
#      127.0.0.1 only.
#   5. Writes Threshold's mobile.json and restarts Threshold if it is running.

set -euo pipefail

PORT=7457
APP_ID=com.thirdbear.threshold
CONFIG_DIR="$HOME/Library/Application Support/$APP_ID"
CONFIG="$CONFIG_DIR/mobile.json"

say()  { printf '\n\033[1m%s\033[0m\n' "$*"; }
fail() { printf '\n\033[31m%s\033[0m\n' "$*" >&2; exit 1; }

[ "$(uname)" = Darwin ] || fail "This script sets up a Mac."

# It asks questions, and installing Tailscale asks for your password. Run
# without a terminal (from an editor, or an agent's shell), both fail
# halfway, so say so before starting anything.
if [ "${1:-}" != --off ] && ! [ -t 0 ]; then
  fail "Run this in a Terminal window: it asks you questions, and installing Tailscale asks for your Mac password."
fi

# --- Tailscale -------------------------------------------------------------

find_tailscale() {
  if command -v tailscale >/dev/null 2>&1; then command -v tailscale; return; fi
  # The standalone and App Store apps carry their CLI inside the bundle.
  local inside=/Applications/Tailscale.app/Contents/MacOS/Tailscale
  [ -x "$inside" ] && echo "$inside"
}

TS="$(find_tailscale || true)"

# Run the Tailscale CLI, but give up after a few seconds. The app's CLI
# waits indefinitely for the app when it is not running or not yet
# approved, and a setup script that hangs silently is worse than one that
# says what it is waiting for.
# The CLI is a shell wrapper around the app's binary that does not exec it,
# so the whole process group is what gets stopped, not just the wrapper.
ts() {
  /usr/bin/perl -e '
    my $t = shift;
    my $pid = fork // die "fork: $!";
    if ($pid == 0) { setpgrp(0, 0); exec @ARGV or exit 127 }
    $SIG{ALRM} = sub { kill "TERM", -$pid; sleep 1; kill "KILL", -$pid; exit 124 };
    alarm $t;
    waitpid($pid, 0);
    exit($? >> 8);
  ' 8 "$TS" "$@"
}

if [ "${1:-}" = --off ]; then
  [ -n "$TS" ] && ts serve --https=443 off >/dev/null 2>&1 || true
  if [ -f "$CONFIG" ]; then
    /usr/bin/python3 - "$CONFIG" <<'PY'
import json, sys
p = sys.argv[1]
c = json.load(open(p))
c["enabled"] = False
json.dump(c, open(p, "w"), indent=2)
PY
  fi
  say "Phone access is off. Restart Threshold for it to stop listening."
  exit 0
fi

if [ -z "$TS" ]; then
  say "Installing Tailscale (the Mac app, from Homebrew)."
  command -v brew >/dev/null 2>&1 || fail "Homebrew is needed to install Tailscale: https://brew.sh"
  brew install --cask tailscale-app
  TS="$(find_tailscale || true)"
  [ -n "$TS" ] || fail "Tailscale installed, but its command-line tool was not found."
fi

# Started first: its CLI can only talk to a running app.
pgrep -xq Tailscale || open -a Tailscale || true

if ! ts status >/dev/null 2>&1; then
  say "Log in to Tailscale."
  cat <<'EOF'
The Tailscale app is open (look for its icon in the menu bar). The first
time, macOS asks you to allow it:
  - "Allow VPN configuration": click Allow.
  - If it says a system extension was blocked: System Settings →
    Privacy & Security, scroll down, click Allow next to Tailscale.
Then log in from the menu-bar icon. Use the same Tailscale account on
every Mac and on your phone.
EOF
  printf 'Waiting for Tailscale to connect (Ctrl-C to stop; re-running picks up here)'
  until ts status >/dev/null 2>&1; do printf '.'; sleep 3; done
  echo
fi

tailnet_json() { ts status --json 2>/dev/null; }

# --- Name ------------------------------------------------------------------

current_name="$(/usr/bin/python3 -c 'import json,sys; print((json.load(open(sys.argv[1])).get("machine") or {}).get("name",""))' "$CONFIG" 2>/dev/null || true)"

say "What should your phone call this Mac?"
echo "Short and unmistakable, like the org or the job it does: acme, studio, client-x."
echo "Lowercase letters, digits and hyphens. It becomes this Mac's Tailscale name."
[ -n "$current_name" ] && echo "Currently: $current_name (press return to keep it)"
while :; do
  read -r -p "> " name
  name="${name:-$current_name}"
  name="$(printf '%s' "$name" | tr '[:upper:]' '[:lower:]')"
  if ! printf '%s' "$name" | grep -Eq '^[a-z0-9]([a-z0-9-]{0,30}[a-z0-9])?$'; then
    echo "Use 1-32 lowercase letters, digits or hyphens, not starting or ending with a hyphen."
    continue
  fi
  # A default like macbook-air-2 is how two Macs end up indistinguishable.
  if printf '%s' "$name" | grep -Eq '^(mac|macbook|macbook-air|macbook-pro|imac|mac-mini|mac-studio|localhost)(-[0-9]+)?$'; then
    echo "That is a default Mac name, which is exactly what makes two Macs hard to tell apart. Pick something that says which one this is."
    continue
  fi
  # Another machine on the tailnet already answering to this name would get
  # this one's address suffixed with -1, and the phone would show a name
  # that is not the address.
  taken="$(tailnet_json | /usr/bin/python3 -c '
import json, sys
d = json.load(sys.stdin)
want = sys.argv[1]
for p in (d.get("Peer") or {}).values():
    if (p.get("HostName") or "").lower() == want or (p.get("DNSName") or "").split(".")[0] == want:
        print(p.get("HostName")); break
' "$name" || true)"
  if [ -n "$taken" ]; then
    echo "Another device on your tailnet is already called $name. Pick a different name."
    continue
  fi
  break
done

# --- Colour ----------------------------------------------------------------

say "Pick a colour for $name."
echo "It runs across the top of every phone screen for this Mac. Give each Mac a different one."
colours="blue:#2f6fdf green:#1f9d55 orange:#e8730c purple:#8a4fd8 red:#d23c3c teal:#0f9a9a pink:#d6408f slate:#5a6b7d"
i=1
for c in $colours; do printf '  %d) %s\n' "$i" "${c%%:*}"; i=$((i+1)); done
while :; do
  read -r -p "> " pick
  hex="$(printf '%s\n' $colours | sed -n "${pick:-0}p" | cut -d: -f2)"
  [ -n "$hex" ] && break
  echo "Type a number from the list."
done

# --- Serve -----------------------------------------------------------------

say "Putting $name on your tailnet."
ts set --hostname="$name"

# Serve needs HTTPS certificates enabled for the tailnet. When they are not,
# `tailscale serve` prints a link to turn them on and waits, so it runs in
# the foreground here where you can see that link.
echo "If Tailscale prints a link to enable HTTPS, open it, turn HTTPS on, and come back."
# No time limit here: when HTTPS is off, serve prints a link and waits for
# it to be turned on, which is the person's move, not a hang.
"$TS" serve --bg --https=443 "http://127.0.0.1:$PORT"

dns="$(tailnet_json | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin)["Self"]["DNSName"].rstrip("."))')"
[ -n "$dns" ] || fail "Could not read this Mac's tailnet address from Tailscale."
case "$dns" in
  "$name".*) ;;
  *) fail "Tailscale named this Mac $dns rather than $name.<tailnet>. Pick another name and re-run." ;;
esac
url="https://$dns"

# --- Threshold config ------------------------------------------------------

mkdir -p "$CONFIG_DIR"
renamed="$(/usr/bin/python3 - "$CONFIG" "$name" "$hex" "$url" "$PORT" <<'PY'
import json, os, sys
path, name, colour, url, port = sys.argv[1:]
try:
    c = json.load(open(path))
except (OSError, ValueError):
    c = {}
m = c.get("machine") or {}
renamed = bool(m.get("name")) and m.get("name") != name
# The id survives a rename: it is what phones quote back on every write.
c.update(enabled=True, port=int(port), url=url,
         machine={"id": m.get("id", ""), "name": name, "color": colour})
tmp = path + ".tmp"
json.dump(c, open(tmp, "w"), indent=2)
os.replace(tmp, path)
print("yes" if renamed else "")
PY
)"

if pgrep -x threshold-desktop >/dev/null; then
  say "Restarting Threshold so it starts serving."
  osascript -e "quit app id \"$APP_ID\"" >/dev/null || true
  for _ in $(seq 1 50); do pgrep -x threshold-desktop >/dev/null || break; sleep 0.2; done
  open -b "$APP_ID" || true
fi

say "Done. $name is at $url"
cat <<EOF

On your phone:
  1. Install Tailscale (Play Store, or App Store on iPhone) and log in with
     the same account.
  2. On this Mac, open Threshold → Phone, and scan the QR code with the camera.
  3. In Chrome, ⋮ → Add to Home screen. The icon is labelled "$name".
     (iPhone: Safari → Share → Add to Home Screen; there, notifications
     only work from the home-screen app.)

Repeat this script on each other Mac with a different name and colour.
Each Mac gets its own home-screen icon.
EOF
[ -n "$renamed" ] && echo "This Mac was renamed, so phones paired under the old name must pair again."
exit 0
