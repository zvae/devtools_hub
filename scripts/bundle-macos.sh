#!/bin/zsh
# 构建 release 并打包为 macOS 应用：target/release/DevTools Hub.app
#
# - LSUIElement = true：与 PasteNow 一样只在菜单栏常驻，不显示 Dock 图标；
# - 打成 .app 后，“辅助功能”权限授予的是这个应用，而不是启动它的终端。
#   使用临时（ad-hoc）签名，每次重新打包后可能需要在系统设置中重新勾选权限。
set -euo pipefail

ROOT=${0:A:h:h}
cd "$ROOT"

cargo build --release -p devtools-hub

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
APP="$ROOT/target/release/DevTools Hub.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/devtools-hub "$APP/Contents/MacOS/devtools-hub"
cp crates/devtools-hub/assets/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>devtools-hub</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundleIdentifier</key>
    <string>dev.devtoolshub.app</string>
    <key>CFBundleName</key>
    <string>DevTools Hub</string>
    <key>CFBundleDisplayName</key>
    <string>DevTools Hub</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>LSMinimumSystemVersion</key>
    <string>12.0</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

codesign --force --sign - "$APP"
echo "已生成：$APP"
