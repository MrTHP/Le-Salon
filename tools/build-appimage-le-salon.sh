#!/bin/sh
# ============================================================================
#  Construit Le-Salon-<version>-x86_64.AppImage
#  Usage : sh tools/build-appimage-le-salon.sh [--no-download]
# ============================================================================
set -eu
APP_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$APP_ROOT"

VERSION="${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)}"
APP=le-salon
APPDIR="$APP_ROOT/AppDir"
OUT="$APP_ROOT/Le-Salon-$VERSION-x86_64.AppImage"

# Vérifie la compatibilité de la glibc du système de build : une AppImage
# hérite de ses exigences et ne peut pas tourner sur une glibc plus ancienne.
GLIBC_VERSION="$(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{print $2}')"
if [ -n "$GLIBC_VERSION" ]; then
  GLIBC_MAJOR="${GLIBC_VERSION%%.*}"
  GLIBC_MINOR="${GLIBC_VERSION#*.}"
  if [ "$GLIBC_MAJOR" -gt 2 ] || { [ "$GLIBC_MAJOR" -eq 2 ] && [ "$GLIBC_MINOR" -gt 35 ]; }; then
    echo "ATTENTION : glibc $GLIBC_VERSION détectée." >&2
    echo "Cette AppImage ne démarrera pas sur Ubuntu 22.04 (glibc 2.35)," >&2
    echo "la plus ancienne LTS encore supportée par le catalogue AppImage." >&2
    echo "Construire dans un conteneur ubuntu:22.04 (voir .github/workflows/build.yml)." >&2
  fi
fi

echo "== compilation release ($APP $VERSION) =="
cargo build --release -p le-salon

echo "== AppDir =="
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/lib" \
         "$APPDIR/usr/share/applications" \
         "$APPDIR/usr/share/metainfo" \
         "$APPDIR/usr/share/icons/hicolor/512x512/apps"

cp -L "$APP_ROOT/target/release/$APP" "$APPDIR/usr/bin/$APP"
cp "$APP_ROOT/packaging/le-salon.desktop" "$APPDIR/$APP.desktop"
cp "$APP_ROOT/packaging/le-salon.desktop" "$APPDIR/usr/share/applications/$APP.desktop"
cp "$APP_ROOT/packaging/org.ircv3.lesalon.appdata.xml" "$APPDIR/usr/share/metainfo/org.ircv3.lesalon.appdata.xml"
cp "$APP_ROOT/packaging/le-salon.png" "$APPDIR/$APP.png"
cp "$APP_ROOT/packaging/le-salon.png" "$APPDIR/.DirIcon"
cp "$APP_ROOT/packaging/le-salon.png" \
   "$APPDIR/usr/share/icons/hicolor/512x512/apps/$APP.png"
cp "$APP_ROOT/packaging/AppRun" "$APPDIR/AppRun"
chmod +x "$APPDIR/AppRun" "$APPDIR/usr/bin/$APP"

echo "== bibliothèques embarquées (ldd) =="
# On n'embarque que les bibliothèques X11/xkb réellement liées. Le reste
# (Vulkan, Wayland, fontconfig, freetype, pilotes) vient de l'hôte : c'est
# indispensable pour utiliser le GPU et les polices de la distribution.
EXCLUDE='^linux-vdso|ld-linux|libc\.so|libm\.so|libpthread|libdl\.so|librt\.so|libnss_|libresolv|libutil|libgcc_s|libstdc\+\+|libvulkan|libGL|libEGL|libdrm|libgbm|libfontconfig|libfreetype|libwayland|^libxcb\.so'
ldd "$APP_ROOT/target/release/$APP" |
  awk '{if ($3 ~ /^\//) print $3; else if ($1 ~ /^\//) print $1}' |
  sort -u |
  while read -r lib; do
    base="$(basename "$lib")"
    if echo "$base" | grep -Eq "$EXCLUDE"; then
      echo "  système : $base"
    else
      echo "  embarquée : $base"
      cp -L "$lib" "$APPDIR/usr/lib/$base"
    fi
  done

# Dépendances des bibliothèques embarquées (libxcb-xkb, etc.).
for lib in "$APPDIR"/usr/lib/*.so*; do
  [ -e "$lib" ] || continue
  ldd "$lib" 2>/dev/null |
    awk '{if ($3 ~ /^\//) print $3; else if ($1 ~ /^\//) print $1}' |
    sort -u |
    while read -r dep; do
      base="$(basename "$dep")"
      if echo "$base" | grep -Eq "$EXCLUDE"; then continue; fi
      if [ ! -e "$APPDIR/usr/lib/$base" ]; then
        echo "  embarquée (dépendance) : $base"
        cp -L "$dep" "$APPDIR/usr/lib/$base"
      fi
    done
done

echo "== contenu AppDir =="
find "$APPDIR" -maxdepth 3 | sort
du -sh "$APPDIR"

# --- appimagetool ---
NEED_DL=1
if command -v appimagetool >/dev/null 2>&1; then NEED_DL=0; fi
if [ "${1:-}" = "--no-download" ]; then NEED_DL=0; fi
if [ "$NEED_DL" = "1" ]; then
  if ! command -v mksquashfs >/dev/null 2>&1; then
    echo "mksquashfs manquant : installez squashfs-tools (sudo dnf install squashfs-tools)." >&2
    exit 1
  fi
  if [ ! -x /tmp/opencode/appimagetool ]; then
    echo "== téléchargement appimagetool =="
    mkdir -p /tmp/opencode
    curl -L -o /tmp/opencode/appimagetool \
      https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
    chmod +x /tmp/opencode/appimagetool
  fi
  APPIMAGETOOL=/tmp/opencode/appimagetool
else
  if [ -x /tmp/opencode/appimagetool ]; then
    APPIMAGETOOL=/tmp/opencode/appimagetool
  else
    APPIMAGETOOL="${APPIMAGETOOL:-appimagetool}"
  fi
fi

echo "== empaquetage : $OUT =="
# Sans FUSE (conteneur, CI), le runtime d'appimagetool doit s'auto-extraire.
if [ ! -e /dev/fuse ] && [ "${APPIMAGE_EXTRACT_AND_RUN:-0}" != "1" ]; then
  export APPIMAGE_EXTRACT_AND_RUN=1
fi
ARCH=x86_64 "$APPIMAGETOOL" "$APPDIR" "$OUT"
chmod +x "$OUT"
ls -lh "$OUT"
echo "Test : $APPDIR/usr/bin/$APP --version"
"$APPDIR/usr/bin/$APP" --version
echo OK.
