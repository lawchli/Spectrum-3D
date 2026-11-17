#!/usr/bin/env bash
# Reuse an existing SDK without copying or changing it. Gradle/user caches stay local.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
SDK=${1:?Usage: scripts/android-sdk-link.sh /absolute/path/to/android-sdk}
if [[ ! -d "$SDK/platforms" || ! -d "$SDK/build-tools" ]]; then echo "Not an Android SDK: $SDK" >&2; exit 1; fi
if [[ -e "$ROOT/toolchains/android-sdk" || -L "$ROOT/toolchains/android-sdk" ]]; then echo "toolchains/android-sdk already exists" >&2; exit 1; fi
ln -s "$SDK" "$ROOT/toolchains/android-sdk"
