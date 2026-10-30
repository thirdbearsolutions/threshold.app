#!/bin/bash
# Install the git hooks that keep the installed Threshold current with this
# checkout: after a commit, merge, rebase or checkout, scripts/auto-install.sh
# runs in the background. See that script for what it does.
#
#   scripts/install-hooks.sh          install (safe to re-run)
#   scripts/install-hooks.sh --remove take them out again

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HOOKS="$(git -C "$ROOT" rev-parse --git-common-dir)/hooks"
case "$HOOKS" in /*) ;; *) HOOKS="$ROOT/$HOOKS" ;; esac
MARK="# threshold auto-install"

for hook in post-commit post-merge post-rewrite post-checkout; do
  path="$HOOKS/$hook"
  if [ "${1:-}" = --remove ]; then
    if [ -f "$path" ] && grep -q "$MARK" "$path"; then rm "$path"; echo "removed $hook"; fi
    continue
  fi
  # Never overwrite a hook somebody else wrote.
  if [ -f "$path" ] && ! grep -q "$MARK" "$path"; then
    echo "$hook already exists and is not ours; add this line to it:" >&2
    echo "  nohup \"$ROOT/scripts/auto-install.sh\" >> \"\$HOME/Library/Logs/threshold-auto-install.log\" 2>&1 &" >&2
    continue
  fi
  cat > "$path" <<EOF
#!/bin/sh
$MARK
# Rebuilds and installs Threshold if this change touched it, in the background, so
# git returns at once. Log: ~/Library/Logs/threshold-auto-install.log
nohup "\$(git rev-parse --show-toplevel)/scripts/auto-install.sh" >> "\$HOME/Library/Logs/threshold-auto-install.log" 2>&1 </dev/null &
exit 0
EOF
  chmod +x "$path"
  echo "installed $hook"
done
