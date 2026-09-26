#!/bin/zsh
# Install opendash as a launchd agent that survives logout and reboot.
#
# Why this script exists: the project lives on an external SSD, and launchd's spawn
# context blocks indefinitely on open() under /Volumes/*. A LaunchAgent pointing at
# the SSD therefore hangs at startup with no output. So the supervised copy of the
# binary and its config both go on the internal volume, while the repo stays where
# it is. Run it again after changing config.toml to re-sync.
#
#   ./launchd/install.sh          install + start
#   ./launchd/install.sh --sync   re-copy config and restart, keep the repo as source
#   ./launchd/install.sh --uninstall

set -eu

PROJECT="$(cd "$(dirname "$0")/.." && pwd)"
LABEL="com.jericle.opendash"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
BIN_SRC="$PROJECT/target/release/opendash"
BIN_DST="$HOME/.local/bin/opendash"
RUNTIME="$HOME/.opendash"
DOMAIN="gui/$(id -u)"

log()  { print -r -- "==> $*" }
fail() { print -r -- "error: $*" >&2; exit 1; }

uninstall() {
  log "unloading $LABEL"
  launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
  rm -f "$PLIST"
  log "removed $PLIST (binary and $RUNTIME left in place)"
}

case "${1:-install}" in
  --uninstall) uninstall; exit 0 ;;
  --sync)
    # Rebuild first: the page is embedded with include_str!, so a stale binary
    # silently serves the previous HTML. Copying without rebuilding is the exact
    # trap that made an edit look like it had not landed.
    command -v cargo >/dev/null 2>&1 && (cd "$PROJECT" && cargo build --release) \
      || log "cargo not found; syncing the existing binary (run cargo build --release yourself)"
    [ -f "$BIN_SRC" ] || fail "no binary at $BIN_SRC — run: cargo build --release"
    launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
    install -m 755 "$BIN_SRC" "$BIN_DST"
    mkdir -p "$RUNTIME"
    # The repo's config.toml stays the source of truth; the runtime copy is what
    # launchd can actually read.
    cp "$PROJECT/config.toml" "$RUNTIME/config.toml"
    log "synced binary and config from $PROJECT"
    ;;
  install)
    command -v cargo >/dev/null 2>&1 && (cd "$PROJECT" && cargo build --release) \
      || log "cargo not found; install the existing binary (run cargo build --release yourself)"
    [ -f "$BIN_SRC" ] || fail "no binary at $BIN_SRC — run: cargo build --release"
    mkdir -p "$HOME/.local/bin" "$RUNTIME" "$HOME/Library/LaunchAgents" "$RUNTIME/data"
    install -m 755 "$BIN_SRC" "$BIN_DST"
    [ -f "$RUNTIME/config.toml" ] || cp "$PROJECT/config.toml" "$RUNTIME/config.toml"
    log "installed binary to $BIN_DST"
    log "runtime config at $RUNTIME/config.toml"
    ;;
  *) fail "usage: $0 [--sync|--uninstall]" ;;
esac

# Generate the plist with absolute paths resolved now, rather than making the user
# edit placeholders.
cat > "$PLIST" <<PLIST_EOF
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$LABEL</string>
    <key>ProgramArguments</key>
    <array>
        <string>$BIN_DST</string>
        <string>serve</string>
        <string>--config</string>
        <string>$RUNTIME/config.toml</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>OPENDASH_CONFIG</key>
        <string>$RUNTIME/config.toml</string>
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>$RUNTIME/data/daemon.log</string>
    <key>StandardErrorPath</key>
    <string>$RUNTIME/data/daemon.err.log</string>
</dict>
</plist>
PLIST_EOF
plutil -lint "$PLIST" >/dev/null || fail "generated an invalid plist"

launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
launchctl bootstrap "$DOMAIN" "$PLIST" || fail "bootstrap failed"
log "loaded $LABEL"

# Give it a moment, then prove it is actually answering rather than trusting it.
sleep 6
PORT=$(awk -F'=' '/^port/{gsub(/ /,"",$2); print $2}' "$RUNTIME/config.toml" 2>/dev/null || echo 8787)
PORT="${PORT:-8787}"
if curl -fsS -m 5 "http://localhost:$PORT/health" >/dev/null 2>&1; then
  log "healthy: http://localhost:$PORT"
  curl -s "http://localhost:$PORT/health"; print
  print
  print -r -- "  logs:   tail -f $RUNTIME/data/daemon.log"
  print -r -- "  stop:   launchctl bootout $DOMAIN/$LABEL"
  print -r -- "  remove: $0 --uninstall"
else
  print -r -- "did not answer on port $PORT; last log lines:" >&2
  tail -20 "$RUNTIME/data/daemon.err.log" 2>/dev/null >&2
  tail -20 "$RUNTIME/data/daemon.log" 2>/dev/null >&2
  fail "startup check failed"
fi
