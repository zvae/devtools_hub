#!/bin/zsh
# 构建 release 并打包为 macOS 应用：target/release/DevTools Hub.app
#
# - LSUIElement = true：与 PasteNow 一样只在菜单栏常驻，不显示 Dock 图标；
# - 打成 .app 后，“辅助功能”权限授予的是这个应用，而不是启动它的终端；
# - 签名：设置了 MACOS_SIGN_IDENTITY（证书名称或 SHA-1，证书所在的钥匙串要在搜索列表中）时用这张证书签名，
#   系统按“Bundle ID + 证书”识别应用，重新打包、更新后权限仍然有效。没有设置时用临时（ad-hoc）签名，
#   系统按代码哈希识别，每次重新打包后都要在“辅助功能”中删掉本应用再重新添加。
#   证书见 scripts/create-signing-cert.sh。
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

sign_args=(--force)
if [[ -n "${MACOS_SIGN_IDENTITY:-}" ]]; then
    # 自签名证书没有可用的时间戳服务器。
    sign_args+=(--sign "$MACOS_SIGN_IDENTITY" --timestamp=none)
else
    echo "未设置 MACOS_SIGN_IDENTITY，使用临时签名：每次重新打包后都要重新授予“辅助功能”权限" >&2
    sign_args+=(--sign -)
fi
codesign "${sign_args[@]}" "$APP"
codesign --verify --strict "$APP"
codesign --display --requirements - "$APP" 2>&1 | grep "designated =>"
echo "已生成：$APP"
