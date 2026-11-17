#!/bin/sh
# Adds the signed APKs from dist/ to Tunnelrat's own F-Droid repository and
# publishes it (GitHub Pages: https://i232826.github.io/tunnelrat-fdroid/repo).
# Needs the repository working copy (with its index signing key, config.yml
# and keystore.p12, which never leave this PC) and Docker for fdroidserver.
#   FDROID_REPO=~/tunnelrat-fdroid FDROIDSERVER=~/fdroid-test/fdroidserver fdroid/update-repo.sh
# SPDX-License-Identifier: GPL-3.0-or-later
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
work=${FDROID_REPO:-$HOME/tunnelrat-fdroid}
server=${FDROIDSERVER:-$HOME/fdroid-test/fdroidserver}
[ -f "$work/config.yml" ] || { echo "No F-Droid repository at $work" >&2; exit 1; }

# Store texts and screenshots come from the fastlane metadata in this repository.
rm -rf "$work/metadata/dev.tunnelrat/en-US"
mkdir -p "$work/metadata/dev.tunnelrat"
cp -r "$here/fastlane/metadata/android/en-US" "$work/metadata/dev.tunnelrat/"

for apk in "$here"/dist/tunnelrat-*.apk; do
    [ -f "$work/repo/$(basename "$apk")" ] || cp "$apk" "$work/repo/"
done

docker run --rm -u vagrant --entrypoint /bin/bash -v "$work":/repo -v "$server":/home/vagrant/fdroidserver \
    registry.gitlab.com/fdroid/fdroidserver:buildserver -c '
. /etc/profile
export PATH="/home/vagrant/fdroidserver:$PATH" PYTHONPATH="/home/vagrant/fdroidserver:/home/vagrant/fdroidserver/examples"
cd /repo && fdroid update --pretty'

cd "$work"
git add -A
git ls-files | grep -qE '^(config\.yml|keystore\.p12)$' && { echo "Refusing: signing files staged" >&2; exit 1; }
git diff --cached --quiet && { echo "Nothing new to publish."; exit 0; }
# Commit as the project, never as whatever identity this machine has.
git -c user.name=anyrat -c user.email=i873248076234896724@gmail.com commit -qm "Update repository

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push -q
echo "Published; GitHub Pages updates in a minute or two."
