#!/usr/bin/env bash
# Ejecuta Nocturne SOLO en el monitor interno (eDP-2, workspace 2) bajo Hyprland 0.56 (Lua).
# Uso: tools/run_on_laptop.sh <segundos> <log> [--shot <seg> <png>]... -- [args del juego]
# Variables: NOCTURNE_BIN (por defecto target/debug/nocturne), NOCTURNE_CLASS (app_id, "nocturne").
# Las capturas se hacen con grim sobre eDP-2 a los <seg> indicados.
set -u
SECS=$1; LOG=$2; shift 2
SHOTS=()
while [ $# -gt 0 ] && [ "$1" = "--shot" ]; do SHOTS+=("$2:$3"); shift 3; done
[ "${1:-}" = "--" ] && shift
PROJ="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${NOCTURNE_BIN:-$PROJ/target/debug/nocturne}"
CLASS="${NOCTURNE_CLASS:-nocturne}"
EXIT="$LOG.exit"; RUNNER="$LOG.sh"; rm -f "$EXIT" "$LOG"
{
  echo "#!/usr/bin/env bash"
  echo "cd '$PROJ'"
  printf "timeout %s %q " "$SECS" "$BIN"; printf '%q ' "$@"; echo "> '$LOG' 2>&1"
  echo "echo \$? > '$EXIT'"
} > "$RUNNER"; chmod +x "$RUNNER"
hyprctl dispatch "hl.dsp.exec_cmd(\"$RUNNER\", { workspace = \"2 silent\" })" >/dev/null
START=$(date +%s%N)
while [ ! -f "$EXIT" ]; do
  hyprctl clients -j | python3 -c '
import json, subprocess, sys
cls = sys.argv[1]
for c in json.load(sys.stdin):
    if c["class"] == cls and c["monitor"] != 1:
        subprocess.run(["hyprctl", "dispatch", "hl.dsp.window.move({ workspace = 2, follow = false, window = \"address:%s\" })" % c["address"]], capture_output=True)
        print("ventana movida a eDP-2", file=sys.stderr)' "$CLASS"
  NOW=$(date +%s%N)
  for i in "${!SHOTS[@]}"; do
    s="${SHOTS[$i]}"; [ -z "$s" ] && continue
    t="${s%%:*}"; f="${s#*:}"
    if (( (NOW - START) / 1000000 >= t * 1000 )); then grim -o eDP-2 "$f"; SHOTS[$i]=""; fi
  done
  sleep 0.2
done
cat "$LOG" | tail -40
exit "$(cat "$EXIT")"
