#!/bin/sh
# ============================================================================
#  Teste le démarrage d'une AppImage sous Xvfb, sans pilote Vulkan.
#  Usage : sh tools/smoke-test-appimage.sh [chemin.AppImage]
# ============================================================================
set -eu
APP_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$APP_ROOT"

APPIMAGE="${1:-$(ls Le-Salon-*-x86_64.AppImage 2>/dev/null | head -1)}"
if [ -z "$APPIMAGE" ]; then
  echo "Aucune AppImage Le-Salon trouvée." >&2
  exit 1
fi
case "$APPIMAGE" in
  /*) ;;
  *) APPIMAGE="$APP_ROOT/$APPIMAGE" ;;
esac

HOME_DIR="$(mktemp -d)"
INNER="$(mktemp)"
trap 'rm -rf "$HOME_DIR" "$INNER"' EXIT

cat > "$INNER" <<'EOF'
#!/bin/sh
set -eu
export APPIMAGE_EXTRACT_AND_RUN=1
"$APPIMAGE" >/tmp/le-salon-smoke.log 2>&1 &
pid=$!
i=0
while [ "$i" -lt 40 ]; do
  i=$((i + 1))
  if ! kill -0 "$pid" 2>/dev/null; then
    echo "L'application a quitté prématurément :" >&2
    cat /tmp/le-salon-smoke.log >&2
    exit 1
  fi
  if xdotool search --onlyvisible --name "Le Salon" >/tmp/le-salon-windows.txt 2>/dev/null; then
    echo "Fenêtre détectée après $((i * 250)) ms."
    kill "$pid" 2>/dev/null || true
    exit 0
  fi
  sleep 0.25
done
echo "Aucune fenêtre Le Salon détectée après 10 s :" >&2
cat /tmp/le-salon-smoke.log >&2
kill "$pid" 2>/dev/null || true
exit 1
EOF
chmod +x "$INNER"

export APPIMAGE
HOME="$HOME_DIR" xvfb-run -a "$INNER"
