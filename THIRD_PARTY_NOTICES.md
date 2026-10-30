# Third-party notices

## git

Threshold.app carries its own copy of git at `Contents/MacOS/git`, so it
runs on a Mac without Apple's Command Line Tools. It's a separate program
that Threshold runs, built unmodified by `scripts/build-git.sh`.

- Version: 2.56.0
- License: GNU General Public License, version 2
- Source: https://mirrors.edge.kernel.org/pub/software/scm/git/git-2.56.0.tar.xz
  (sha256 `26c56c296b38c0695b26fa95f475f1d01704d2d38e73465ca30b0b2f5dc789d3`)

The build options are in `scripts/build-git.sh`.
