#!/bin/zsh
# Pre-open briefing loop.
#
# Deliberately NOT a launchd StartCalendarInterval: the ET->local offset is +14h
# while the US is on EDT and +13h on EST, and Sydney's own DST moves on a
# different schedule. A hardcoded local time would be wrong twice a year, so this
# asks the binary for the next 08:25 ET instant, sleeps until then, fires the
# macOS notification, and re-arms.
#
# Launch with:  launchd/preopen_alert.sh loop     (foreground, Ctrl-C to stop)
#           or: launchd/preopen_alert.sh once     (fire if the time has arrived)
#           or: launchd/preopen_alert.sh now      (fire immediately, ignore the clock)

set -u

DIR="${OPENDASH_DIR:-$(cd "$(dirname "$0")/.." && pwd)}"
BIN="$DIR/target/release/opendash"
URL="${OPENDASH_URL:-http://127.0.0.1:8787}"

log() { print -r -- "[$(date '+%Y-%m-%d %H:%M:%S %Z')] $*" }

# If the daemon is not up, the refresh inside the binary still works (it fetches
# directly), but starting it means the dashboard is already open in the browser.
ensure_daemon() {
  if ! curl -fsS -m 3 "$URL/health" >/dev/null 2>&1; then
    log "daemon not responding on $URL, starting it"
    (cd "$DIR" && nohup "$BIN" serve >"$DIR/data/daemon.log" 2>&1 &)
    sleep 3
  fi
}

fire() {
  log "firing pre-open briefing"
  ensure_daemon
  (cd "$DIR" && "$BIN" alert-once)
  open "$URL" 2>/dev/null || true
  log "done"
}

if [[ ! -x "$BIN" ]]; then
  log "binary not found at $BIN — run: cargo build --release"
  exit 1
fi

case "${1:-loop}" in
  now)
    fire
    ;;
  once)
    next=$("$BIN" next-alert | sed -n 's/^next_alert_epoch=//p')
    now=$(date +%s)
    if [[ -n "$next" && "$next" -le $((now + 60)) ]]; then
      fire
    else
      log "not yet time; next briefing at epoch $next"
    fi
    ;;
  loop)
    log "watching for 08:25 ET briefings (dir=$DIR)"
    while true; do
      info=$("$BIN" next-alert 2>/dev/null)
      next=$(print -r -- "$info" | sed -n 's/^next_alert_epoch=//p')
      et=$(print -r -- "$info" | sed -n 's/^next_alert_et=//p')
      loc=$(print -r -- "$info" | sed -n 's/^next_alert_local=//p')
      if [[ -z "$next" ]]; then
        log "could not read next alert time; retrying in 60s"
        sleep 60
        continue
      fi
      now=$(date +%s)
      wait=$((next - now))
      log "next briefing: $et  (= $loc), sleeping ${wait}s"
      (( wait > 0 )) && sleep "$wait"
      fire
      # Re-arm: next_alert_instant is strictly "after now", so this cannot re-fire
      # for today's already-past 08:25.
      sleep 90
    done
    ;;
  *)
    echo "usage: $0 [loop|once|now]" >&2
    exit 2
    ;;
esac
