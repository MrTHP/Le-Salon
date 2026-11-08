#!/bin/sh
# Test de bout en bout du socle ircv3-core contre tools/fake_ircd.py.
# Couvre : TCP clair, TLS auto-signé, CAP, JOIN, messages et refus de SASL
# sans TLS.
set -eu
APP_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REPO_ROOT="$(cd "$APP_ROOT/.." && pwd)"
cd "$APP_ROOT"

PIDS=""
cleanup() {
  for pid in $PIDS; do
    kill "$pid" 2>/dev/null || true
  done
}
trap cleanup EXIT INT TERM

echo "== compilation du probe =="
cargo build -p ircv3-core --example probe

run_case() {
  name="$1"
  port="$2"
  server_args="$3"
  probe_args="$4"
  extra_grep="$5"

  echo "== cas : $name (port $port) =="
  python3 "$REPO_ROOT/tools/fake_ircd.py" "$port" $server_args \
    > "/tmp/opencode/ircv3-e2e-$name.log" 2>&1 &
  PIDS="$PIDS $!"
  sleep 0.6
  timeout 14 ./target/debug/examples/probe \
    --host 127.0.0.1 --port "$port" --nick probe --channel '#test' \
    --seconds 8 $probe_args > "/tmp/opencode/ircv3-probe-$name.log" 2>&1
  grep -q "bonjour tout le monde" "/tmp/opencode/ircv3-probe-$name.log"
  grep -q "fait un essai de rendu" "/tmp/opencode/ircv3-probe-$name.log"
  grep -q "message avec id" "/tmp/opencode/ircv3-probe-$name.log"
  if [ -n "$extra_grep" ]; then
    grep -q "$extra_grep" "/tmp/opencode/ircv3-probe-$name.log"
  fi
  echo "   OK"
}

run_case clair 6680 "" "--no-tls" "Sujet : Salon de test"
run_case tls 6690 "--tls" "--tls --insecure" "Sujet : Salon de test"

echo "== cas : refus SASL sans TLS (port 6681) =="
python3 "$REPO_ROOT/tools/fake_ircd.py" 6681 \
  > "/tmp/opencode/ircv3-e2e-sasl-refus.log" 2>&1 &
PIDS="$PIDS $!"
sleep 0.6
timeout 14 ./target/debug/examples/probe \
  --host 127.0.0.1 --port 6681 --nick probe --channel '#test' \
  --seconds 3 --no-tls --sasl-user compte --sasl-pass secret \
  > "/tmp/opencode/ircv3-probe-sasl-refus.log" 2>&1 || true
grep -q "SASL" "/tmp/opencode/ircv3-probe-sasl-refus.log"
grep -q "TLS" "/tmp/opencode/ircv3-probe-sasl-refus.log"
echo "   OK"

echo "Tous les tests de bout en bout sont passés."
