#!/usr/bin/env bash
# Shared, data-only Fleet configuration. Call fleet_load_config <runtime root>.
fleet_load_config() {
  local runtime="$1" allow_missing="${2:-0}" default_binary="${3:-}" file line key value digest config_home= config_state= config_telemetry=
  file="${XDG_CONFIG_HOME:-$HOME/.config}/zoe-fleet/config"
  if [ -f "$file" ]; then
    while IFS= read -r line || [ -n "$line" ]; do
      case "$line" in ''|\#*) continue ;; esac
      case "$line" in *=*) ;; *) printf 'Fleet: invalid config line in %s\n' "$file" >&2; return 1 ;; esac
      key=${line%%=*}
      value=${line#*=}
      case "$key" in
        FM_HOME) config_home=$value; [ "${FM_HOME+x}" = x ] || FM_HOME=$value ;;
        ZOE_FLEET_BIN) [ "${ZOE_FLEET_BIN+x}" = x ] || ZOE_FLEET_BIN=$value ;;
        ZOE_FLEET_STATE_DIR) config_state=$value ;;
        ZOE_TELEMETRY_DIR) config_telemetry=$value ;;
        ZOE_NO_MISTAKES_BIN) [ "${ZOE_NO_MISTAKES_BIN+x}" = x ] || ZOE_NO_MISTAKES_BIN=$value ;;
        ZOE_FLEET_RUNTIME) [ "${ZOE_FLEET_RUNTIME+x}" = x ] || ZOE_FLEET_RUNTIME=$value ;;
        *) printf 'Fleet: unknown config key %s in %s\n' "$key" "$file" >&2; return 1 ;;
      esac
    done < "$file"
  fi
  if [ "$allow_missing" = 1 ]; then
    ZOE_FLEET_RUNTIME=${ZOE_FLEET_RUNTIME:-$runtime}
    default_binary=${default_binary:-$ZOE_FLEET_RUNTIME/bin/zoe-fleet}
    ZOE_FLEET_BIN=${ZOE_FLEET_BIN:-$default_binary}
    export ZOE_FLEET_RUNTIME ZOE_FLEET_BIN
    return 0
  fi
  if [ -z "${FM_HOME:-}" ]; then
    printf 'Fleet: FM_HOME is unset; run zoe-fleet-tower setup --home <Firstmate home>.\n' >&2
    return 1
  fi
  if [ ! -d "$FM_HOME" ]; then
    printf 'Fleet: FM_HOME is not a directory: %s\n' "$FM_HOME" >&2
    return 1
  fi
  FM_HOME=$(cd "$FM_HOME" && pwd -P)
  if [ -n "$config_home" ] && [ "$(cd "$config_home" 2>/dev/null && pwd -P)" = "$FM_HOME" ]; then
    [ "${ZOE_FLEET_STATE_DIR+x}" = x ] || [ -z "$config_state" ] || ZOE_FLEET_STATE_DIR=$config_state
    [ "${ZOE_TELEMETRY_DIR+x}" = x ] || [ -z "$config_telemetry" ] || ZOE_TELEMETRY_DIR=$config_telemetry
  fi
  ZOE_FLEET_RUNTIME=${ZOE_FLEET_RUNTIME:-$runtime}
  default_binary=${default_binary:-$ZOE_FLEET_RUNTIME/bin/zoe-fleet}
  ZOE_FLEET_BIN=${ZOE_FLEET_BIN:-$default_binary}
  if [ -z "${ZOE_FLEET_STATE_DIR:-}" ]; then
    digest=$(python3 -c 'import hashlib,sys; print(hashlib.sha256(sys.argv[1].encode()).hexdigest()[:12])' "$FM_HOME") || return 1
    ZOE_FLEET_STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/zoe-fleet/$digest"
  fi
  ZOE_TELEMETRY_DIR=${ZOE_TELEMETRY_DIR:-$ZOE_FLEET_STATE_DIR/telemetry}
  export FM_HOME ZOE_FLEET_RUNTIME ZOE_FLEET_BIN ZOE_FLEET_STATE_DIR ZOE_TELEMETRY_DIR
  [ -z "${ZOE_NO_MISTAKES_BIN:-}" ] || export ZOE_NO_MISTAKES_BIN
}
