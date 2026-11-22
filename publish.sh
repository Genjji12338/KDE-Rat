#!/bin/sh
# Publishes the current files of this branch (usually main) to GitHub as one
# new commit on the `public` branch, whose history starts clean at 0.2.0, so
# the private development history never leaves this PC.
#   ./publish.sh "message"          then optionally: git tag -a vX.Y.Z public
# SPDX-License-Identifier: GPL-3.0-only
set -eu
msg=${1:?usage: ./publish.sh "commit message"}
[ -z "$(git status --porcelain)" ] || { echo "Commit or stash your changes first." >&2; exit 1; }
tree=$(git rev-parse "HEAD^{tree}")
[ "$tree" != "$(git rev-parse "public^{tree}")" ] || { echo "public already has these files."; exit 0; }
c=$(git -c user.name=anyrat -c user.email=i873248076234896724@gmail.com commit-tree "$tree" -p public -m "$msg" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>")
git branch -f public "$c"
git push github
git log --oneline -1 public
