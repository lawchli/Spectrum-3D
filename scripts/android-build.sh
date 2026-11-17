#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
# Prefer an explicit override, then a compatible existing home, then managed Java 17 installations.
java17() {
  [[ -x "$1/bin/java" ]] || return 1
  local spectrum_java_version
  spectrum_java_version=$("$1/bin/java" -version 2>&1)
  [[ "$spectrum_java_version" == *'version "17.'* ]]
}
SPECTRUM_BUILD_JAVA=""
if [[ -n "${SPECTRUM_JAVA_HOME:-}" ]]; then
  SPECTRUM_BUILD_JAVA="$SPECTRUM_JAVA_HOME"
elif [[ -n "${JAVA_HOME:-}" ]] && java17 "$JAVA_HOME"; then
  SPECTRUM_BUILD_JAVA="$JAVA_HOME"
else
  if [[ -x /usr/libexec/java_home ]]; then SPECTRUM_BUILD_JAVA=$(/usr/libexec/java_home -v 17 2>/dev/null || true); fi
  if ! java17 "$SPECTRUM_BUILD_JAVA" && command -v mise >/dev/null 2>&1; then
    SPECTRUM_BUILD_JAVA=$(mise where java@17 2>/dev/null || true)
  fi
  if ! java17 "$SPECTRUM_BUILD_JAVA"; then
    for candidate in "$HOME"/.local/share/mise/installs/java/*17* "$HOME"/.jdks/*17*; do
      if java17 "$candidate"; then SPECTRUM_BUILD_JAVA="$candidate"; break; fi
    done
  fi
fi
if ! java17 "$SPECTRUM_BUILD_JAVA"; then
  echo "Java 17 missing: set SPECTRUM_JAVA_HOME to an installed Java 17 JDK" >&2
  exit 1
fi
export JAVA_HOME="$SPECTRUM_BUILD_JAVA"
export ANDROID_HOME="${SPECTRUM_ANDROID_HOME:-$ROOT/toolchains/android-sdk}"
export ANDROID_SDK_ROOT="$ANDROID_HOME"
export ANDROID_USER_HOME="$ROOT/toolchains/android-user"
export GRADLE_USER_HOME="$ROOT/toolchains/gradle"
export PATH="$JAVA_HOME/bin:$PATH"
if [[ ! -d "$ANDROID_HOME/platforms" ]]; then
  echo "Android SDK missing: set SPECTRUM_ANDROID_HOME to an existing SDK or provision toolchains/android-sdk" >&2
  exit 1
fi
cd "$ROOT/android"
if [[ $# -eq 0 ]]; then set -- :spectrum:assembleRelease :app:assembleDebug; fi
exec ./gradlew "$@"
