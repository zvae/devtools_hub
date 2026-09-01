#!/bin/zsh
set -euo pipefail

SCRIPT_DIR="${0:A:h}"
PROJECT_ROOT="${SCRIPT_DIR:h:h}"
APP_PATH="${PROJECT_ROOT}/target/release/DevTools Hub.app"

cargo build --release -p devtools-app --manifest-path "${PROJECT_ROOT}/Cargo.toml"

rm -rf "${APP_PATH}"
mkdir -p "${APP_PATH}/Contents/MacOS" "${APP_PATH}/Contents/Resources"
cp "${PROJECT_ROOT}/target/release/devtools-app" "${APP_PATH}/Contents/MacOS/DevToolsHub"
cp "${SCRIPT_DIR}/Info.plist" "${APP_PATH}/Contents/Info.plist"
cp "${PROJECT_ROOT}/assets/DevToolsHub.icns" "${APP_PATH}/Contents/Resources/DevToolsHub.icns"

echo "Built ${APP_PATH}"
