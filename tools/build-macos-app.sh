#!/bin/sh
# ============================================================================
#  Construit « Le Salon.app » (universal arm64 + x86_64 si possible) et le
#  compresse en zip. À exécuter sur macOS (Metal/Xcode requis par GPUI).
# ============================================================================
set -eu
APP_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$APP_ROOT"

VERSION="${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)}"
BUNDLE="$APP_ROOT/dist/Le Salon.app"
OUT="$APP_ROOT/Le-Salon-$VERSION-macos-universal.zip"

if [ "$(uname -s)" != "Darwin" ]; then
  echo "Ce script doit tourner sur macOS (GPUI utilise Metal)." >&2
  exit 1
fi

echo "== compilation release =="
HOST="$(rustc -vV | sed -n 's/^host: //p')"
if rustup target list --installed | grep -q '^aarch64-apple-darwin$' &&
   rustup target list --installed | grep -q '^x86_64-apple-darwin$'; then
  cargo build --release -p le-salon --target aarch64-apple-darwin
  cargo build --release -p le-salon --target x86_64-apple-darwin
  mkdir -p "$APP_ROOT/dist"
  lipo -create \
    "$APP_ROOT/target/aarch64-apple-darwin/release/le-salon" \
    "$APP_ROOT/target/x86_64-apple-darwin/release/le-salon" \
    -output "$APP_ROOT/dist/le-salon-universal"
  BIN="$APP_ROOT/dist/le-salon-universal"
else
  echo "Cibles universelles absentes ; construction native ($HOST)." >&2
  echo "Pour l'universel : rustup target add aarch64-apple-darwin x86_64-apple-darwin" >&2
  cargo build --release -p le-salon
  BIN="$APP_ROOT/target/release/le-salon"
fi

echo "== bundle .app =="
rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources"
cp "$BIN" "$BUNDLE/Contents/MacOS/le-salon"
cp "$APP_ROOT/packaging/le-salon.icns" "$BUNDLE/Contents/Resources/le-salon.icns"
sed "s/__VERSION__/$VERSION/g" "$APP_ROOT/packaging/Info.plist" \
  > "$BUNDLE/Contents/Info.plist"
chmod +x "$BUNDLE/Contents/MacOS/le-salon"
codesign --force --deep --sign - "$BUNDLE" >/dev/null
echo "Signature ad-hoc appliquée (pas de notarisation Apple)."

echo "== zip =="
rm -f "$OUT"
ditto -c -k --sequesterRsrc --keepParent "$BUNDLE" "$OUT"
ls -lh "$OUT"

echo "Sur le Mac :"
echo "  unzip \"$(basename "$OUT")\""
echo "  xattr -dr com.apple.quarantine \"Le Salon.app\"   # si Gatekeeper bloque"
echo "  open \"Le Salon.app\""
