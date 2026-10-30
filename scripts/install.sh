#!/bin/bash
# Build Threshold from HEAD and install it into /Applications.
#
#   scripts/install.sh [--check] [--allow-dirty] [--stage] [--finish] [threshold]
#
# Threshold is the only app; naming it is optional. --check builds nothing: it reports
# whether each installed app was built from the newest commit touching it.
# --stage builds, but leaves a running app alone: the new build waits in
# the staging directory until --finish installs it once the app has quit.
#
# Each install records the commit it was built from (and a hash of the
# installed binary) under ~/Library/Application Support/threshold/installed/, so
# --check can tell "installed from HEAD" apart from "installed a while ago".

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ALL_APPS="threshold"
LEDGER="$HOME/Library/Application Support/threshold/installed"
# Builds waiting for their app to quit, one bundle and one record per app.
# The record is "<commit>\t<repo root>\t<subject>", read by the app itself
# to offer a restart into the new build.
STAGED="$HOME/Library/Application Support/threshold/staged"
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

check=0
allow_dirty=0
stage=0
finish=0
apps=""
for arg in "$@"; do
  case "$arg" in
    --check) check=1 ;;
    --allow-dirty) allow_dirty=1 ;;
    --stage) stage=1 ;;
    --finish) finish=1 ;;
    -h|--help) sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) echo "unknown option: $arg" >&2; exit 2 ;;
    *)
      case " $ALL_APPS " in
        *" $arg "*) apps="$apps $arg" ;;
        *) echo "unknown app: $arg (expected one of: $ALL_APPS)" >&2; exit 2 ;;
      esac ;;
  esac
done
[ -n "$apps" ] || apps="$ALL_APPS"

app_dir() {
  echo "$ROOT/app"
}

# Everything an app's build reads from, relative to the repo root.
app_sources() {
  echo "app crates/$1-core Cargo.toml Cargo.lock"
}

conf() { # conf <app> <key> -> value of a top-level string key in tauri.conf.json
  sed -n "s/^  \"$2\": \"\(.*\)\",\{0,1\}$/\1/p" "$(app_dir "$1")/src-tauri/tauri.conf.json"
}

binary_name() {
  sed -n '/^\[\[bin\]\]$/,/^name = /s/^name = "\(.*\)"$/\1/p' "$(app_dir "$1")/src-tauri/Cargo.toml"
}

# Newest commit that touched any of the app's sources.
latest_commit() {
  # shellcheck disable=SC2046
  git -C "$ROOT" log -1 --format=%H -- $(app_sources "$1")
}

report() { # report <app>: one line saying whether the installed app is current
  local app="$1" name bin installed record rec_commit rec_hash latest behind
  name="$(conf "$app" productName)"
  bin="$(binary_name "$app")"
  installed="/Applications/$name.app/Contents/MacOS/$bin"
  record="$LEDGER/$app"

  if [ ! -x "$installed" ]; then
    printf '%-12s not installed\n' "$app"; return 1
  fi
  if [ ! -f "$record" ]; then
    printf '%-12s installed, but not by this script (unknown commit)\n' "$app"; return 1
  fi
  read -r rec_commit rec_hash < "$record"
  if [ "$(shasum -a 256 "$installed" | cut -d' ' -f1)" != "$rec_hash" ]; then
    printf '%-12s installed binary differs from the one this script recorded\n' "$app"; return 1
  fi
  case "$rec_commit" in *+dirty)
    printf '%-12s built from uncommitted changes on top of %s\n' "$app" "$(git -C "$ROOT" log -1 --format=%h "${rec_commit%+dirty}")"
    return 1 ;;
  esac
  latest="$(latest_commit "$app")"
  if [ "$rec_commit" = "$latest" ] || git -C "$ROOT" merge-base --is-ancestor "$latest" "$rec_commit" 2>/dev/null; then
    printf '%-12s current (%s)\n' "$app" "$(git -C "$ROOT" log -1 --format='%h %s' "$rec_commit")"
    return 0
  fi
  # shellcheck disable=SC2046
  behind="$(git -C "$ROOT" rev-list --count "$rec_commit..HEAD" -- $(app_sources "$app") 2>/dev/null || echo '?')"
  printf '%-12s out of date: %s commit(s) since %s\n' "$app" "$behind" "$(git -C "$ROOT" log -1 --format=%h "$rec_commit")"
  return 1
}

if [ "$check" = 1 ]; then
  status=0
  for app in $apps; do report "$app" || status=1; done
  exit "$status"
fi

[ "$(uname)" = Darwin ] || { echo "installing into /Applications is macOS-only" >&2; exit 1; }

# Put a bundle in place of the installed one and record what it is.
place() { # place <app> <bundle> <commit>
  local app="$1" from="$2" commit="$3" name bin dest
  name="$(conf "$app" productName)"
  bin="$(binary_name "$app")"
  dest="/Applications/$name.app"
  rm -rf "$dest.installing"
  ditto "$from" "$dest.installing"
  rm -rf "$dest"
  mv "$dest.installing" "$dest"
  "$LSREGISTER" -f "$dest"
  mkdir -p "$LEDGER"
  echo "$commit $(shasum -a 256 "$dest/Contents/MacOS/$bin" | cut -d' ' -f1)" > "$LEDGER/$app"
}

if [ "$finish" = 1 ]; then
  # Install whatever was staged for an app that has since quit, and reopen
  # it if it quit in order to update.
  for app in $apps; do
    [ -d "$STAGED/$app.app" ] || continue
    bin="$(binary_name "$app")"
    if pgrep -x "$bin" >/dev/null; then echo "$app: still running; staged build waits"; continue; fi
    commit="$(cut -f1 "$STAGED/$app")"
    echo "==> $app: installing the staged build ($commit)"
    place "$app" "$STAGED/$app.app" "$commit"
    rm -rf "$STAGED/$app.app" "$STAGED/$app"
    if [ -e "$STAGED/$app.restart" ]; then
      rm -f "$STAGED/$app.restart"
      open "/Applications/$(conf "$app" productName).app"
    fi
    report "$app" || true
  done
  exit 0
fi

# An install should correspond to a commit, or --check can't vouch for it.
if [ "$allow_dirty" = 0 ]; then
  for app in $apps; do
    # shellcheck disable=SC2046
    if [ -n "$(git -C "$ROOT" status --porcelain -- $(app_sources "$app"))" ]; then
      echo "$app has uncommitted changes; commit them or pass --allow-dirty" >&2
      exit 1
    fi
  done
fi

mkdir -p "$LEDGER"
head="$(git -C "$ROOT" rev-parse HEAD)"

for app in $apps; do
  dir="$(app_dir "$app")"
  name="$(conf "$app" productName)"
  identifier="$(conf "$app" identifier)"
  bin="$(binary_name "$app")"
  built="$ROOT/target/release/bundle/macos/$name.app"
  dest="/Applications/$name.app"

  echo "==> $name: building from $(git -C "$ROOT" log -1 --format='%h %s' HEAD)"
  [ -d "$dir/node_modules" ] || (cd "$dir" && npm ci)
  "$ROOT/scripts/build-git.sh"
  (cd "$dir" && npm run app:build -- --bundles app)

  commit="$head"
  # shellcheck disable=SC2046
  [ -z "$(git -C "$ROOT" status --porcelain -- $(app_sources "$app"))" ] || commit="$head+dirty"

  if [ "$stage" = 1 ] && pgrep -x "$bin" >/dev/null; then
    # Someone may be in the middle of using it. The build waits beside it
    # until it quits, or until it asks to restart into it.
    echo "==> $name: running, so staging the build instead of installing it"
    mkdir -p "$STAGED"
    rm -rf "$STAGED/$app.app"
    ditto "$built" "$STAGED/$app.app"
    printf '%s\t%s\t%s\n' "$commit" "$ROOT" "$(git -C "$ROOT" log -1 --format=%s "$head")" > "$STAGED/$app"
    continue
  fi
  # A direct install supersedes anything staged earlier.
  rm -rf "$STAGED/$app.app" "$STAGED/$app"

  was_running=0
  if pgrep -x "$bin" >/dev/null; then
    was_running=1
    echo "==> $name: quitting the running copy"
    osascript -e "quit app id \"$identifier\"" >/dev/null
    for _ in $(seq 1 50); do pgrep -x "$bin" >/dev/null || break; sleep 0.2; done
    if pgrep -x "$bin" >/dev/null; then
      echo "$name did not quit; close it and re-run" >&2
      exit 1
    fi
  fi

  echo "==> $name: installing to $dest"
  place "$app" "$built" "$commit"

  if ! cmp -s "$dest/Contents/MacOS/$bin" "$ROOT/target/release/$bin"; then
    echo "$name: installed binary does not match target/release/$bin" >&2
    exit 1
  fi

  [ "$was_running" = 0 ] || open "$dest"
  report "$app"
done
