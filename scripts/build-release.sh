#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

bash ./gradlew --no-daemon --max-workers=2 \
    :app:lintNonRootRelease :app:testNonRootDebugUnitTest :app:testRootDebugUnitTest
bash ./gradlew --no-daemon --max-workers=2 :app:assembleNonRootRelease :app:bundleNonRootRelease

artifacts=(
    app/build/outputs/apk/nonRoot/release/app-nonRoot-release.apk
    app/build/outputs/bundle/nonRootRelease/app-nonRoot-release.aab
)
sha256sum "${artifacts[@]}"
wc -c "${artifacts[@]}"
