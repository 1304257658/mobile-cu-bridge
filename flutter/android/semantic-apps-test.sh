#!/usr/bin/env bash
# JUnit 4 for SemanticAppsTest. No Android SDK.
# Requires java on PATH. Downloads Kotlin 1.9.10, JUnit 4.13.2, and Hamcrest 1.3.
set -euo pipefail

if ! command -v java >/dev/null 2>&1; then
  echo "java is required on PATH" >&2
  exit 1
fi

root="$(cd "$(dirname "$0")/../.." && pwd)"
cache="${TMPDIR:-/tmp}/semantic-apps-deps"
mkdir -p "$cache"

kotlin_zip="$cache/kotlin-compiler-1.9.10.zip"
junit_jar="$cache/junit-4.13.2.jar"
hamcrest_jar="$cache/hamcrest-core-1.3.jar"

download() {
  local url="$1" dest="$2" sum="$3"
  if [[ -f "$dest" ]] && echo "$sum  $dest" | sha256sum -c --status; then
    return
  fi
  curl -fsSL -o "$dest.partial" "$url"
  mv "$dest.partial" "$dest"
  echo "$sum  $dest" | sha256sum -c --status
}

download \
  "https://github.com/JetBrains/kotlin/releases/download/v1.9.10/kotlin-compiler-1.9.10.zip" \
  "$kotlin_zip" \
  "7d74863deecf8e0f28ea54c3735feab003d0eac67e8d3a791254b16889c20342"
download \
  "https://repo1.maven.org/maven2/junit/junit/4.13.2/junit-4.13.2.jar" \
  "$junit_jar" \
  "8e495b634469d64fb8acfa3495a065cbacc8a0fff55ce1e31007be4c16dc57d3"
download \
  "https://repo1.maven.org/maven2/org/hamcrest/hamcrest-core/1.3/hamcrest-core-1.3.jar" \
  "$hamcrest_jar" \
  "66fdef91e9739348df7a096aa384a5685f4e875584cce89386a7a47251c4d8e9"

if [[ ! -x "$cache/kotlinc/bin/kotlinc" ]]; then
  unzip -q -o "$kotlin_zip" -d "$cache"
fi

out="$cache/classes"
rm -rf "$out"
mkdir -p "$out"

"$cache/kotlinc/bin/kotlinc" \
  -cp "$junit_jar" \
  -d "$out" \
  "$root/flutter/android/app/src/main/kotlin/com/carriez/flutter_hbb/SemanticApps.kt" \
  "$root/flutter/android/app/src/test/kotlin/com/carriez/flutter_hbb/SemanticAppsTest.kt"

exec java -cp "$out:$junit_jar:$hamcrest_jar:$cache/kotlinc/lib/kotlin-stdlib.jar" \
  org.junit.runner.JUnitCore \
  com.carriez.flutter_hbb.SemanticAppsTest
