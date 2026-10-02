#!/bin/bash
# One-off: create the key that signs updates. Run it yourself on this Mac,
# never in CI.
#   1. generates a minisign key pair, protected by a password you choose;
#   2. writes the public key into src-tauri/tauri.conf.json (commit it);
#   3. stores the private key and its password in the repo's Actions secrets
#      TAURI_SIGNING_PRIVATE_KEY / TAURI_SIGNING_PRIVATE_KEY_PASSWORD.
# The app only installs updates signed with this key. If it is lost, installed
# copies can't update themselves any more: back up the file and the password.
#   scripts/setup-updater-keys.sh [key file, default ~/.tauri/moliwhisper-updater.key]
source "$(dirname "$0")/lib.sh"

KEY="${1:-$HOME/.tauri/moliwhisper-updater.key}"
CONF="src-tauri/tauri.conf.json"

if [[ -e "$KEY" ]]; then
  echo "❌ $KEY already exists; pass another path." >&2
  exit 1
fi
command -v gh >/dev/null || { echo "❌ needs gh (https://cli.github.com)" >&2; exit 1; }

read -rsp "Password for the new key: " PASSWORD; echo
read -rsp "Again: " AGAIN; echo
if [[ -z "$PASSWORD" || "$PASSWORD" != "$AGAIN" ]]; then
  echo "❌ The passwords are empty or differ." >&2
  exit 1
fi

mkdir -p "$(dirname "$KEY")"
pnpm -s tauri signer generate --ci -w "$KEY" -p "$PASSWORD" >/dev/null
chmod 600 "$KEY"
PUBKEY="$(cat "$KEY.pub")"
[[ -n "$PUBKEY" ]] || { echo "❌ no public key in $KEY.pub" >&2; exit 1; }

export PUBKEY
perl -pi -e 's/"pubkey": "[^"]*"/"pubkey": "$ENV{PUBKEY}"/' "$CONF"
grep -q "\"pubkey\": \"$PUBKEY\"" "$CONF" || { echo "❌ could not write the public key into $CONF" >&2; exit 1; }

gh secret set TAURI_SIGNING_PRIVATE_KEY -R "$REPO" < "$KEY"
printf '%s' "$PASSWORD" | gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD -R "$REPO"

cat <<EOF
✅ Public key written to $CONF
✅ Private key: $KEY (secrets TAURI_SIGNING_PRIVATE_KEY / _PASSWORD set on $REPO)

Next:
  1. Back up $KEY and its password somewhere safe (a password manager);
  2. Commit $CONF.
EOF
