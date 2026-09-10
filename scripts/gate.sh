#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

CONTRACTS=(
  contracts/hos-wallet
  contracts/wallet-impl-deployer
  contracts/registrar
  contracts/hos-extension
  contracts/mpc-recovery
  contracts/tla-registry
  dev-contracts/test-ft
  dev-contracts/test-mpc
  dev-contracts/test-staking-pool
  dev-contracts/test-dapp
)

MAINNET_RPC=${MAINNET_RPC:-https://rpc.mainnet.near.org}
VERIFIER_ACCOUNT=${VERIFIER_ACCOUNT:-intents.near}
VERIFIER_WASM=$PWD/target/intents-mainnet.wasm

step() { printf '\n== %s\n' "$1"; }

rpc() {
  curl -sS --max-time 60 -X POST "$MAINNET_RPC" -H 'Content-Type: application/json' -d "$1"
}

fetch_deployed_verifier() {
  local query hash cached
  query='{"jsonrpc":"2.0","id":1,"method":"query","params":{"request_type":"view_account","finality":"final","account_id":"'$VERIFIER_ACCOUNT'"}}'
  if ! hash=$(rpc "$query" | jq -er '.result.code_hash'); then
    printf '  cannot reach %s to read %s\n' "$MAINNET_RPC" "$VERIFIER_ACCOUNT" >&2
    printf '  settlement is only proven against the deployed verifier; fix the network\n' >&2
    printf '  rather than skipping this step, or the suite proves nothing about mainnet\n' >&2
    return 1
  fi
  printf '  %s code hash %s\n' "$VERIFIER_ACCOUNT" "$hash"
  cached=$(cat "$VERIFIER_WASM.hash" 2>/dev/null || true)
  if [ "$hash" = "$cached" ] && [ -s "$VERIFIER_WASM" ]; then
    printf '  cached\n'
    return 0
  fi
  printf '  fetching\n'
  query='{"jsonrpc":"2.0","id":1,"method":"query","params":{"request_type":"view_code","finality":"final","account_id":"'$VERIFIER_ACCOUNT'"}}'
  rpc "$query" | jq -er '.result.code_base64' | base64 -d >"$VERIFIER_WASM"
  [ -s "$VERIFIER_WASM" ] || { printf '  empty wasm from %s\n' "$MAINNET_RPC" >&2; return 1; }
  printf '%s' "$hash" >"$VERIFIER_WASM.hash"
  printf '  %s bytes\n' "$(stat -c%s "$VERIFIER_WASM")"
}

INTEGRATION_TARGET=${INTEGRATION_TARGET:-$PWD/target/integration}

integration() {
  (cd integration && CARGO_TARGET_DIR=$INTEGRATION_TARGET "$@")
}

step "workspace fmt"
cargo fmt --all --check

step "integration fmt"
integration cargo fmt --check

step "advisories"
if ! command -v cargo-audit >/dev/null; then
  printf '  cargo-audit is missing, so the accepted advisories in .cargo/audit.toml are a\n' >&2
  printf '  claim nothing checks. Install it with cargo install cargo-audit rather than\n' >&2
  printf '  skipping this step\n' >&2
  exit 1
fi
cargo audit

step "workspace clippy"
cargo clippy --workspace --all-targets -- -D warnings

step "integration clippy"
integration cargo clippy --all-targets -- -D warnings

step "workspace unit tests"
cargo test --workspace --lib

step "contract wasm"
for contract in "${CONTRACTS[@]}"; do
  printf '  %s\n' "$contract"
  (cd "$contract" && cargo near build non-reproducible-wasm --locked --no-abi)
done

step "integration tests"
integration cargo test

step "deployed verifier"
fetch_deployed_verifier

step "settlement against the deployed verifier"
(cd integration && DEFUSE_WASM=$VERIFIER_WASM CARGO_TARGET_DIR=$INTEGRATION_TARGET \
  cargo test --test intents_e2e)

printf '\ngate passed\n'
printf 'release builds are reproducible-wasm; see README before tagging\n'
