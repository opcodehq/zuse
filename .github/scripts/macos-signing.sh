#!/bin/bash
set -euo pipefail
if [ -z "$CSC_LINK" ]; then
  echo "::error::CSC_LINK is required for macOS releases"
  exit 1
fi
keychain="$RUNNER_TEMP/zuse-signing.keychain-db"
keychain_password=$(uuidgen)
echo "$CSC_LINK" | base64 --decode > "$RUNNER_TEMP/certificate.p12"
security create-keychain -p "$keychain_password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$keychain_password" "$keychain"
security import "$RUNNER_TEMP/certificate.p12" -k "$keychain" -P "$CSC_KEY_PASSWORD" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple: -s -k "$keychain_password" "$keychain" >/dev/null
keychains=()
while IFS= read -r line; do
  line="${line#"${line%%[![:space:]]*}"}"; line="${line#\"}"; line="${line%\"}"
  [ -n "$line" ] && keychains+=("$line")
done < <(security list-keychains -d user)
security list-keychains -d user -s "$keychain" "${keychains[@]}"
rm "$RUNNER_TEMP/certificate.p12"
identity=$(security find-identity -v -p codesigning "$keychain" | grep -o '"Developer ID Application: [^"]*"' | head -1 | tr -d '"')
if [ -z "$identity" ]; then
  echo "::error::The certificate in CSC_LINK has no Developer ID Application identity"
  exit 1
fi
echo "ZUSE_MACOS_SIGN_IDENTITY=$identity" >> "$GITHUB_ENV"
echo "ZUSE_MACOS_KEYCHAIN=$keychain" >> "$GITHUB_ENV"
