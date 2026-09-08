#!/bin/sh

set -eu

readonly route_state="${ROUTE_STATE:-}"
readonly operation_store_root="${OPERATION_STORE_ROOT:-/data/operations}"
readonly interval_seconds="${MONITOR_INTERVAL_SECONDS:-60}"
readonly success_marker="${MONITOR_SUCCESS_MARKER:-/tmp/oft-monitor-last-success}"

log() {
  printf '%s %s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$*" >&2
}

validate_environment() {
  if [ -z "$route_state" ]; then
    log "ROUTE_STATE is required"
    return 64
  fi
  if [ ! -d "$route_state" ]; then
    log "route state directory is missing: $route_state"
    return 66
  fi
  if [ ! -r "$route_state/route.json" ]; then
    log "authoritative route state is not readable: $route_state/route.json"
    return 66
  fi
  if [ ! -d "$operation_store_root" ] || [ ! -r "$operation_store_root" ]; then
    log "operation store is missing or unreadable: $operation_store_root"
    return 66
  fi
  case "$interval_seconds" in
    ''|*[!0-9]*|0)
      log "MONITOR_INTERVAL_SECONDS must be a positive integer"
      return 64
      ;;
  esac
}

run_once() {
  log "checking route health: $route_state"
  if ! tmplr-oft-bridge health --state "$route_state"; then
    log "route health check failed"
    return 1
  fi

  log "checking custody reconciliation: $route_state"
  if ! tmplr-oft-bridge reconcile --state "$route_state" --fail-on-deficit; then
    log "custody reconciliation failed"
    return 1
  fi

  : > "$success_marker"
  log "route health and custody reconciliation passed"
}

validate_environment

case "${1:---loop}" in
  --once)
    run_once
    ;;
  --loop)
    trap 'log "monitor stopped"; exit 0' INT TERM
    while :; do
      run_once || exit $?
      sleep "$interval_seconds"
    done
    ;;
  *)
    log "usage: monitor-route.sh [--once|--loop]"
    exit 64
    ;;
esac
