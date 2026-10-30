#!/usr/bin/env bash
# Build the git that ships inside Threshold.app.
#
# macOS's /usr/bin/git is a stub until Apple's Command Line Tools are
# installed, so Threshold carries its own. This builds it from a pinned,
# checksummed release into app/src-tauri/binaries/git, which the bundle
# copies to Contents/MacOS/git. Building it needs the Command Line Tools;
# running the app afterwards doesn't. Does nothing if it's already built.
#
#   scripts/build-git.sh           build if missing
#   scripts/build-git.sh --force   rebuild
set -euo pipefail

VERSION=2.56.0
SHA256=26c56c296b38c0695b26fa95f475f1d01704d2d38e73465ca30b0b2f5dc789d3

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/app/src-tauri/binaries/git"

if [ "${1:-}" != "--force" ] && [ -x "$OUT" ] && "$OUT" --version | grep -qx "git version $VERSION"; then
  exit 0
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
TARBALL="git-$VERSION.tar.xz"

echo "==> git $VERSION: downloading"
curl -fsSL -o "$WORK/$TARBALL" "https://mirrors.edge.kernel.org/pub/software/scm/git/$TARBALL"
echo "$SHA256  $WORK/$TARBALL" | shasum -a 256 -c - >/dev/null
tar -xf "$WORK/$TARBALL" -C "$WORK"

echo "==> git $VERSION: building"
# Only what Threshold runs: diff, rev-parse, show, init, add, commit. No
# network, Perl, Python, Tcl or translations, and no libraries beyond the
# ones every Mac has.
(cd "$WORK/git-$VERSION" && MACOSX_DEPLOYMENT_TARGET=11.0 make -j"$(sysctl -n hw.ncpu)" git \
  prefix=/usr/local \
  NO_GETTEXT=1 NO_PERL=1 NO_PYTHON=1 NO_TCLTK=1 NO_CURL=1 NO_EXPAT=1 NO_OPENSSL=1 \
  CFLAGS=-O2 >"$WORK/build.log" 2>&1) || { tail -20 "$WORK/build.log" >&2; exit 1; }

mkdir -p "$(dirname "$OUT")"
strip -o "$OUT" "$WORK/git-$VERSION/git"
# strip invalidates the linker's signature, and an unsigned binary won't
# run on Apple silicon. Sign it ad hoc, as the app is.
codesign --force --sign - --options runtime "$OUT"
"$OUT" --version
