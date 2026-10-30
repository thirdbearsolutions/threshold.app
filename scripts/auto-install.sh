#!/bin/bash
# Keep the installed Threshold current with this checkout, in the
# background. Started by the git hooks that scripts/install-hooks.sh puts in
# place (after a commit, merge, rebase or checkout), and by an app asking to
# restart into its staged build.
#
# For each installed app whose sources changed since it was installed: build
# it, and install it at once if it is not running. If it is running, the
# build is staged beside it rather than quitting it on someone mid-use, and
# this keeps watching until the app quits, then installs the staged build.
# Apps never installed are left alone.
#
# Only the main checkout counts. A commit in another worktree is somebody's
# branch, not what should be on this Mac.
#
# Log: ~/Library/Logs/threshold-auto-install.log

set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
STATE="$HOME/Library/Application Support/threshold"
LOCK="$STATE/auto-install.lock"
AGAIN="$STATE/auto-install.again"
STAGED="$STATE/staged"
INSTALL="$ROOT/scripts/install.sh"

# The main checkout is the one whose git dir is the common one.
[ "$(git -C "$ROOT" rev-parse --git-dir)" = "$(git -C "$ROOT" rev-parse --git-common-dir)" ] || exit 0
# Mid-rebase, every replayed commit fires the hook; the final one is enough.
gitdir="$(git -C "$ROOT" rev-parse --absolute-git-dir)"
[ -d "$gitdir/rebase-merge" ] || [ -d "$gitdir/rebase-apply" ] && exit 0

mkdir -p "$STATE"
# One runner at a time. A second start while one runs asks it to look
# again when it finishes, so a burst of commits is one build, not five.
if ! mkdir "$LOCK" 2>/dev/null; then
  # A lock left by a runner that no longer exists is not a runner.
  if [ -f "$LOCK/pid" ] && kill -0 "$(cat "$LOCK/pid")" 2>/dev/null; then
    touch "$AGAIN"
    exit 0
  fi
  rm -rf "$LOCK"
  mkdir "$LOCK" || exit 0
fi
echo $$ > "$LOCK/pid"
trap 'rm -rf "$LOCK"' EXIT

log() { printf '%s %s\n' "$(date '+%F %T')" "$*"; }

# Apps installed and out of date with the checkout.
stale() {
  "$INSTALL" --check 2>/dev/null | awk '/ out of date: / { print $1 }'
}

build_pass() {
  local app
  local head
  head="$(git -C "$ROOT" rev-parse HEAD)"
  for app in $(stale); do
    # Already built from this commit and waiting for the app to quit:
    # building it again would change nothing. Commits a second apart used
    # to cost two builds.
    if [ -f "$STAGED/$app" ] && [ "$(cut -f1 "$STAGED/$app")" = "$head" ]; then
      continue
    fi
    log "$app is out of date; building"
    # Refuses when the app's sources have uncommitted changes, which is
    # right: an install should be a commit. The next commit catches up.
    "$INSTALL" --stage "$app" || log "$app: not installed this time (see above)"
  done
}

log "started in $ROOT at $(git -C "$ROOT" log -1 --format='%h %s')"
touch "$AGAIN"
# Install anything staged whose app has quit. First in each round, so an
# app that quit in order to update gets the build it was offered, and is
# reopened, before any rebuild could replace it.
finish_pass() {
  waiting=0
  for record in "$STAGED"/*; do
    [ -f "$record" ] || continue
    app="$(basename "$record")"
    case "$app" in *.restart|*.app) continue ;; esac
    "$INSTALL" --finish "$app" | grep -v 'staged build waits' || true
    [ -f "$record" ] && waiting=1
  done
}

while :; do
  finish_pass
  if [ -e "$AGAIN" ]; then
    rm -f "$AGAIN"
    build_pass
    finish_pass
  fi
  [ "$waiting" = 1 ] || [ -e "$AGAIN" ] || break
  sleep 3
done
log "done"
