#!/usr/bin/env bash
# Live end-to-end exercise of the proxy-oracle stack against real Reflector,
# RedStone and Pyth Lazer contracts. Phases are resumable through a state file.
#
#   NET=testnet SRC=<identity> PYTH_LAZER_API_KEY_FILE=~/.config/templar/pyth-lazer.key scripts/e2e_live.sh all
#   scripts/e2e_live.sh deploy | configure | push | refresh | all
#
# Mainnet writes require ALLOW_MAINNET_WRITE=1.
set -euo pipefail

PHASE="${1:-all}"
NET="${NET:-testnet}"
SRC="${SRC:?identity name (stellar keys ls)}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
WASM_DIR="${WASM_DIR:-$ROOT/target/proxy-oracle-soroban/wasm}"
OUT="${OUT:-$ROOT/target/proxy-oracle-soroban/e2e/$NET}"
STATE="$OUT/state.json"
LOG="$OUT/transactions.log"

ASSET_SYMBOL="${ASSET_SYMBOL:-XLM}"
ASSET_JSON="{\"Other\":\"$ASSET_SYMBOL\"}"
BASE_JSON='{"Other":"USD"}'
LAZER_FEED_ID="${LAZER_FEED_ID:-23}"
MAX_AGE_SECS="${MAX_AGE_SECS:-600}"
MAX_CLOCK_DRIFT_SECS="${MAX_CLOCK_DRIFT_SECS:-60}"
LAZER_CHANNEL="${LAZER_CHANNEL:-fixed_rate@200ms}"
case "$LAZER_CHANNEL" in
  real_time) LAZER_CHANNEL_VARIANT=RealTime ;;
  fixed_rate@50ms) LAZER_CHANNEL_VARIANT=FixedRate50ms ;;
  fixed_rate@200ms) LAZER_CHANNEL_VARIANT=FixedRate200ms ;;
  fixed_rate@1000ms) LAZER_CHANNEL_VARIANT=FixedRate1000ms ;;
  *) echo "unsupported LAZER_CHANNEL $LAZER_CHANNEL" >&2; exit 1 ;;
esac
LAZER_REST="${LAZER_REST:-https://pyth-lazer.dourolabs.app/v1/latest_price}"

case "$NET" in
  testnet)
    PYTH_VERIFIER="${PYTH_VERIFIER:-CAYFT5JE3UQTKT4Q6ZOZK4FXVYVT6RE3MFC7STA4UB6WAEGBT65MRU52}"
    REFLECTOR="${REFLECTOR:-CCYOZJCOPG34LLQQ7N24YXBM7LL62R7ONMZ3G6WZAAYPB5OYKOMJRN63}"
    REDSTONE="${REDSTONE:-CA7MY6TYNL5Z5H5FYGMN7YWSY3JIZG7LFY3DZ26EEGRBQ2UKTFWHD4ZJ}"
    REDSTONE_ASSET_DEFAULT='{"Stellar":"CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC"}'
    RPC_URL="${RPC_URL:-https://soroban-testnet.stellar.org}"
    PASSPHRASE="Test SDF Network ; September 2015"
    FEE_ARGS=()
    ;;
  mainnet)
    [[ "${ALLOW_MAINNET_WRITE:-0}" == "1" ]] || { echo "mainnet write blocked; set ALLOW_MAINNET_WRITE=1" >&2; exit 1; }
    PYTH_VERIFIER="${PYTH_VERIFIER:-CACZ3GBAKUPIAFRILUFO27J5RUH5GJ2VSJ46LP6GJYSKGDRTQ5MS3HCH}"
    REFLECTOR="${REFLECTOR:-CAFJZQWSED6YAWZU3GWRTOCNPPCGBN32L7QV43XX5LZLFTK6JLN34DLN}"
    REDSTONE="${REDSTONE:-CBMGLKUQZVSAIL5CPDDAWSUY7MAKXISHMOZEVLMBUWBMFGHRJSR4WYRF}"
    REDSTONE_ASSET_DEFAULT='{"Stellar":"CAS3J7GYLGXMF6TDJBBYYSE3HQ6BBSMLNUQ34T6TZMYMW2EVH34XOWMA"}'
    RPC_URL="${RPC_URL:-https://mainnet.sorobanrpc.com}"
    PASSPHRASE="Public Global Stellar Network ; September 2015"
    # Mainnet ledgers run near capacity; base-fee bids get evicted.
    FEE_ARGS=(--fee "${FEE_STROOPS:-1000000}")
    ;;
  *) echo "NET must be testnet or mainnet" >&2; exit 1 ;;
esac
REDSTONE_ASSET_JSON="${REDSTONE_ASSET_JSON:-$REDSTONE_ASSET_DEFAULT}"

mkdir -p "$OUT"
[[ -f "$STATE" ]] || echo '{}' > "$STATE"
export RUST_LOG=off

state_get() { python3 -c "import json,sys; print(json.load(open('$STATE')).get('$1',''))"; }
state_set() {
  python3 -c "
import json, sys
s = json.load(open('$STATE')); s[sys.argv[1]] = sys.argv[2]; json.dump(s, open('$STATE', 'w'), indent=2)" "$1" "$2"
}

# Run a stellar command, tee the full output to the log, echo only the result line(s).
run() {
  echo "\$ stellar $*" >> "$LOG"
  local out
  out="$(stellar "$@" 2>&1)" || { echo "$out" | tee -a "$LOG" >&2; return 1; }
  echo "$out" >> "$LOG"
  echo "$out" | grep -E 'Transaction hash|Signing transaction' | sed 's/^/    /' >&2 || true
  echo "$out" | grep -vE '^(ℹ️|🔗|✅|⚠️|🌎|📦|🔐|📝|  )' | tail -1
}

NET_ARGS=(--rpc-url "$RPC_URL" --network-passphrase "$PASSPHRASE" --source "$SRC")
inv() { run contract invoke "${NET_ARGS[@]}" "${FEE_ARGS[@]}" --id "$@"; }
view() { run contract invoke "${NET_ARGS[@]}" --send no --id "$@"; }
deploy() { run contract deploy "${NET_ARGS[@]}" "${FEE_ARGS[@]}" "$@"; }
# Deploy and record the id under `key`; a failed deploy aborts instead of storing "".
deploy_into() {
  local key="$1" id; shift
  id="$(deploy "$@")" || return 1
  state_set "$key" "$id"
}

src_address() { stellar keys address "$SRC"; }
latest_ledger() {
  curl -s -X POST "$RPC_URL" -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getLatestLedger"}' | python3 -c "import sys,json; print(json.load(sys.stdin)['result']['sequence'])"
}

phase_deploy() {
  local admin; admin="$(src_address)"
  state_set admin "$admin"
  echo "== upload wasms"
  local name pkg
  for pkg in contract governance_contract sep40_adapter_contract pyth_lazer_source_contract batcher_contract; do
    name="hash_$pkg"
    [[ -n "$(state_get "$name")" ]] && continue
    local hash
    hash="$(run contract upload "${NET_ARGS[@]}" "${FEE_ARGS[@]}" --wasm "$WASM_DIR/templar_proxy_oracle_soroban_$pkg.optimized.wasm")" || return 1
    state_set "$name" "$hash"
    echo "  $pkg -> $hash"
  done

  echo "== runtime (bootstrap owner = $SRC)"
  [[ -n "$(state_get runtime)" ]] || deploy_into runtime --wasm-hash "$(state_get hash_contract)" -- --governance "$admin" --base "$BASE_JSON"
  local rt; rt="$(state_get runtime)"; echo "  RT=$rt"

  echo "== governance (admin = $SRC, uniform ttl 0)"
  [[ -n "$(state_get governance)" ]] || deploy_into governance --wasm-hash "$(state_get hash_governance_contract)" -- --admin "$admin" --proxy_oracle "$rt" --initial_uniform_ttl_ns 0
  local gov; gov="$(state_get governance)"; echo "  GOV=$gov"

  echo "== hand runtime ownership to governance"
  local owner; owner="$(view "$rt" -- get_owner)"
  if [[ "$owner" != "\"$gov\"" ]]; then
    inv "$rt" -- transfer_ownership --new_owner "$gov" --live_until_ledger "$(( $(latest_ledger) + 1000000 ))" >/dev/null
    local id; id="$(view "$gov" -- next_proposal_id)"
    inv "$gov" -- create_proposal --caller "$admin" --id "$id" --operation '"AcceptOwnership"' --requested_ttl 0 >/dev/null
    inv "$gov" -- execute_proposal --caller "$admin" --id "$id" >/dev/null
    owner="$(view "$rt" -- get_owner)"
  fi
  echo "  runtime owner: $owner"
  echo "  governance.proxy_oracle: $(view "$gov" -- proxy_oracle)"

  echo "== pyth lazer source"
  if [[ -z "$(state_get lazer_source)" ]]; then
    local config="{\"verifier\":\"$PYTH_VERIFIER\",\"base\":$BASE_JSON,\"decimals\":8,\"channel\":\"$LAZER_CHANNEL_VARIANT\",\"freshness\":{\"max_age_secs\":$MAX_AGE_SECS,\"max_ahead_secs\":5}}"
    local mappings="[{\"feed_id\":$LAZER_FEED_ID,\"asset\":$ASSET_JSON}]"
    deploy_into lazer_source --wasm-hash "$(state_get hash_pyth_lazer_source_contract)" -- --owner "$admin" --config "$config" --feed_mappings "$mappings"
  fi
  echo "  LZ=$(state_get lazer_source)"

  echo "== batcher"
  [[ -n "$(state_get batcher)" ]] || deploy_into batcher --wasm-hash "$(state_get hash_batcher_contract)"
  echo "  BATCH=$(state_get batcher)"

  echo "== sep40 adapter ($ASSET_SYMBOL, decimals 8, resolution 1)"
  [[ -n "$(state_get adapter)" ]] || deploy_into adapter --wasm-hash "$(state_get hash_sep40_adapter_contract)" -- --owner "$admin" --parent_oracle "$rt" --asset "$ASSET_JSON" --decimals 8 --resolution 1 --base "$BASE_JSON"
  echo "  AD=$(state_get adapter)"
}

sanity() {
  echo "  $1 base=$(view "$1" -- base) decimals=$(view "$1" -- decimals) lastprice=$(view "$1" -- lastprice --asset "$2")"
}

phase_configure() {
  local rt gov admin; rt="$(state_get runtime)"; gov="$(state_get governance)"; admin="$(state_get admin)"
  local lz; lz="$(state_get lazer_source)"
  echo "== source sanity (base / decimals / lastprice)"
  sanity "$REFLECTOR" "$ASSET_JSON"
  sanity "$REDSTONE" "$REDSTONE_ASSET_JSON"
  sanity "$lz" "$ASSET_JSON"
  echo "== SetProxy($ASSET_SYMBOL): reflector + redstone + lazer, min 3, max_age $MAX_AGE_SECS"
  local config="{\"sources\":[{\"oracle\":\"$REFLECTOR\",\"asset\":$ASSET_JSON},{\"oracle\":\"$REDSTONE\",\"asset\":$REDSTONE_ASSET_JSON},{\"oracle\":\"$lz\",\"asset\":$ASSET_JSON}],\"min_sources\":3,\"max_age_secs\":$MAX_AGE_SECS,\"max_clock_drift_secs\":$MAX_CLOCK_DRIFT_SECS}"
  local id; id="$(view "$gov" -- next_proposal_id)"
  inv "$gov" -- create_proposal --caller "$admin" --id "$id" --operation "{\"SetProxy\":[$ASSET_JSON,$config]}" --requested_ttl 0 >/dev/null
  inv "$gov" -- execute_proposal --caller "$admin" --id "$id" >/dev/null
  echo "  get_proxy: $(view "$rt" -- get_proxy --asset "$ASSET_JSON")"
}

fetch_lazer_payload() {
  if [[ -z "${PYTH_LAZER_API_KEY:-}" && -n "${PYTH_LAZER_API_KEY_FILE:-}" ]]; then
    PYTH_LAZER_API_KEY="$(tr -d '[:space:]' < "$PYTH_LAZER_API_KEY_FILE")"
  fi
  : "${PYTH_LAZER_API_KEY:?Pyth Lazer API key required for the push phase (PYTH_LAZER_API_KEY or PYTH_LAZER_API_KEY_FILE)}"
  curl -sf -X POST "$LAZER_REST" \
    -H "Authorization: Bearer $PYTH_LAZER_API_KEY" -H 'Content-Type: application/json' \
    -d "{\"priceFeedIds\":[$LAZER_FEED_ID],\"properties\":[\"price\",\"exponent\",\"feedUpdateTimestamp\"],\"formats\":[\"leEcdsa\"],\"channel\":\"$LAZER_CHANNEL\",\"jsonBinaryEncoding\":\"hex\"}" \
    | tee "$OUT/lazer_response.json" \
    | python3 -c "import sys,json; r=json.load(sys.stdin); print(r['leEcdsa']['data'])"
}

phase_push() {
  local lz; lz="$(state_get lazer_source)"
  echo "== fetch leEcdsa payload for feed $LAZER_FEED_ID"
  local payload; payload="$(fetch_lazer_payload)"
  echo "  ${#payload} hex chars"
  echo "== update_price_feeds"
  echo "  stored feeds: $(inv "$lz" -- update_price_feeds --payload "$payload")"
  echo "  stored_price: $(view "$lz" -- stored_price --asset "$ASSET_JSON")"
  echo "  lastprice:    $(view "$lz" -- lastprice --asset "$ASSET_JSON")"
}

phase_refresh() {
  local rt ad batch; rt="$(state_get runtime)"; ad="$(state_get adapter)"; batch="$(state_get batcher)"
  echo "== refresh via runtime"
  echo "  refresh:           $(inv "$rt" -- refresh --asset "$ASSET_JSON")"
  echo "  aggregated_latest: $(view "$rt" -- aggregated_latest --asset "$ASSET_JSON")"
  echo "  adapter lastprice: $(view "$ad" -- lastprice --asset "$ASSET_JSON")"
  echo "== batcher"
  echo "  refresh_many:         $(inv "$batch" -- refresh_many --oracle "$rt" --assets "[$ASSET_JSON]")"
  echo "  extend_ttl_many:      $(inv "$batch" -- extend_ttl_many --oracle "$rt" --assets "[$ASSET_JSON]")"
  echo "  extend_ttl_contracts: $(inv "$batch" -- extend_ttl_contracts --contracts "[\"$(state_get governance)\",\"$ad\",\"$(state_get lazer_source)\"]")"
}

case "$PHASE" in
  deploy) phase_deploy ;;
  configure) phase_configure ;;
  push) phase_push ;;
  refresh) phase_refresh ;;
  all) phase_deploy; phase_configure; phase_push; phase_refresh ;;
  *) echo "unknown phase $PHASE" >&2; exit 1 ;;
esac
echo "state: $STATE"
