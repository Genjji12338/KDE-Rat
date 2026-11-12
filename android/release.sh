#!/bin/sh
# Builds the signed release APK into dist/tunnelrat-<version>.apk.
# The signing key lives outside the repository (see app/build.gradle.kts).
# SPDX-License-Identifier: GPL-3.0-or-later
set -eu
here=$(cd "$(dirname "$0")" && pwd)
cd "$here"
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
./gradlew --quiet assembleRelease
apk=app/build/outputs/apk/release/app-release.apk
if [ ! -f "$apk" ]; then
    echo "Built unsigned (no signing key found): $(ls app/build/outputs/apk/release/*.apk)" >&2
    exit 1
fi
apksigner=$(ls -d "$ANDROID_HOME"/build-tools/*/apksigner | sort -V | tail -1)
"$apksigner" verify --print-certs "$apk" | grep -E "SHA-256" | head -1
version=$(sed -n 's/.*versionName = "\(.*\)".*/\1/p' app/build.gradle.kts)
mkdir -p ../dist
cp "$apk" "../dist/tunnelrat-$version.apk"
echo "../dist/tunnelrat-$version.apk ($(du -h "../dist/tunnelrat-$version.apk" | cut -f1)), sha256 $(sha256sum "../dist/tunnelrat-$version.apk" | cut -c1-16)…"
