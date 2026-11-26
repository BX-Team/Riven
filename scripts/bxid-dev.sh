#!/usr/bin/env bash
# Sandboxed launchers signed in to the local BX ID in BX-Team/code (`bun dev` in apps/id and apps/skins): scripts/bxid-dev.sh [user…]
set -euo pipefail
cd "$(dirname "$0")/.."
[ -n "${IN_NIX_SHELL:-}" ] || exec nix develop -c "$0" "$@"

users=("$@")
[ ${#users[@]} -gt 0 ] || users=(alice bob)
url=${RIVEN_BXID_URL:-http://localhost:8787}
skins=${RIVEN_SKINS_URL:-http://localhost:8788}
# Not $TMPDIR: nix develop points it at a fresh directory every time.
root=${BXID_DEV_HOME:-/tmp/bxid-dev}

cargo build -q
trap 'kill $(jobs -p) 2>/dev/null' EXIT
port=47700
for user in "${users[@]}"; do
  home=$root/$user
  # The token matches the session apps/id/scripts/setup.ts seeds for this user.
  RIVEN_BXID_URL=$url RIVEN_SKINS_URL=$skins XDG_DATA_HOME=$home/data XDG_CONFIG_HOME=$home/config \
    RIVEN_BXID_TOKEN="dev${user}token" RIVEN_LINK_PORT=$port target/debug/riven &
  port=$((port + 1))
done
wait
