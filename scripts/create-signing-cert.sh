#!/bin/zsh
# 生成一张自签名的代码签名证书，给 macOS 包签名用（见 README 的“macOS 签名”）。
#
# 系统按“Bundle ID + 证书”识别用证书签名的应用：只要一直用同一张证书，重新打包、更新后
# “辅助功能”权限仍然有效。证书只需要生成一次，之后不要更换，换了证书用户要重新授权，
# 应用内更新也会拒绝签名不一致的安装包。
#
# 用法：zsh scripts/create-signing-cert.sh [--import] [输出目录]
#   输出目录默认 ~/.devtools-hub-signing（放在仓库外，免得误提交私钥），生成：
#     devtools-hub-signing.p12      证书和私钥（PKCS#12）
#     devtools-hub-signing.p12.b64  上面文件的 base64，作为 GitHub Secret MACOS_CERTIFICATE_P12
#     password.txt                  .p12 的密码，作为 GitHub Secret MACOS_CERTIFICATE_PASSWORD
#   密码可以用环境变量 P12_PASSWORD 指定，不指定时随机生成。
#   --import：同时导入登录钥匙串，本地打包时设置 MACOS_SIGN_IDENTITY="DevTools Hub Self-Signed" 即可。
set -euo pipefail

NAME="DevTools Hub Self-Signed"
import=false
if [[ "${1:-}" == "--import" ]]; then
    import=true
    shift
fi
OUT=${1:-$HOME/.devtools-hub-signing}
P12="$OUT/devtools-hub-signing.p12"

if [[ -e "$P12" ]]; then
    echo "$P12 已经存在。证书不要轻易更换；确实要重新生成请先删掉它。" >&2
    exit 1
fi
mkdir -p "$OUT"
chmod 700 "$OUT"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

cat > "$work/cert.cnf" <<CNF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
subjectKeyIdentifier = hash
CNF

# 用系统自带的 LibreSSL：它生成的 .p12 用 3DES / SHA-1 加密，`security import` 能直接导入；
# Homebrew 的 OpenSSL 3 默认用 AES，较早的 macOS 导入会失败。有效期 20 年。
export P12_PASSWORD=${P12_PASSWORD:-$(/usr/bin/openssl rand -hex 16)}
/usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 7300 \
    -config "$work/cert.cnf" -keyout "$work/key.pem" -out "$work/cert.pem" 2>/dev/null
/usr/bin/openssl pkcs12 -export -name "$NAME" \
    -inkey "$work/key.pem" -in "$work/cert.pem" \
    -out "$P12" -passout env:P12_PASSWORD
base64 -i "$P12" > "$P12.b64"
print -rn -- "$P12_PASSWORD" > "$OUT/password.txt"
chmod 600 "$P12" "$P12.b64" "$OUT/password.txt"

echo "已生成证书：$NAME"
/usr/bin/openssl x509 -in "$work/cert.pem" -noout -fingerprint -sha1
echo
echo "在 GitHub 仓库的 Settings › Secrets and variables › Actions 中添加："
echo "  MACOS_CERTIFICATE_P12       = $P12.b64 的内容"
echo "  MACOS_CERTIFICATE_PASSWORD  = $OUT/password.txt 的内容"
echo
echo "这几个文件包含私钥，请妥善备份（例如放进密码管理器），不要提交到仓库。"

if $import; then
    security import "$P12" -k "$HOME/Library/Keychains/login.keychain-db" \
        -P "$P12_PASSWORD" -T /usr/bin/codesign
    echo
    echo "已导入登录钥匙串。本地打包：MACOS_SIGN_IDENTITY=\"$NAME\" zsh scripts/bundle-macos.sh"
    echo "第一次签名时系统会询问是否允许 codesign 使用这把私钥，选“始终允许”。"
fi
