#!/bin/sh
# TokenMeter installer: fetch the latest release, verify SHA256SUMS, install both binaries.
#   curl -fsSL https://raw.githubusercontent.com/helloimdevman/tokenmeter/main/install.sh | sh
set -eu

REPO="${TOKENMETER_REPO:-helloimdevman/tokenmeter}"
DIR="${TOKENMETER_INSTALL_DIR:-$HOME/.local/bin}"
BASE="https://github.com/$REPO/releases/latest/download"

case "$(uname -s)/$(uname -m)" in
  Darwin/arm64) TAG=macos-arm64 ;;
  Darwin/x86_64) TAG=macos-x64 ;;
  Linux/x86_64) TAG=linux-x64 ;;
  Linux/aarch64 | Linux/arm64) TAG=linux-arm64 ;;
  *) echo "tokenmeter: unsupported platform $(uname -s)/$(uname -m) (macOS and Linux only)" >&2; exit 1 ;;
esac

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

for asset in "tokenmeter-$TAG" "tokenmeter-hook-$TAG" SHA256SUMS; do
  curl -fsSL -o "$TMP/$asset" "$BASE/$asset"
done

# SHA256SUMS lists every platform: check only our two files, and require both.
grep -E "[ *](tokenmeter|tokenmeter-hook)-$TAG\$" "$TMP/SHA256SUMS" > "$TMP/sums" || true
if [ "$(grep -c . "$TMP/sums")" -ne 2 ]; then
  echo "tokenmeter: SHA256SUMS has no entries for $TAG" >&2
  exit 1
fi
if command -v sha256sum > /dev/null 2>&1; then
  (cd "$TMP" && sha256sum -c sums)
else
  (cd "$TMP" && shasum -a 256 -c sums)
fi

mkdir -p "$DIR"
for name in tokenmeter tokenmeter-hook; do
  cp "$TMP/$name-$TAG" "$DIR/.$name.new"
  chmod 755 "$DIR/.$name.new"
  mv -f "$DIR/.$name.new" "$DIR/$name"
done

"$DIR/tokenmeter" install

case ":$PATH:" in
  *":$DIR:"*) ;;
  *) echo; echo "Add $DIR to your PATH:"; echo "  export PATH=\"$DIR:\$PATH\"" ;;
esac
