#!/usr/bin/env bash
# 15 §4.7; 09 §7.1 — adapt Zombienet's Substrate-node arguments to B9.
# The final exec is required: native pause/resume signals the PID Zombienet
# tracks, so that PID must become bleavit-keeper rather than remain a wrapper.
set -euo pipefail

keeper_node_url=""
keeper_signer_uri=""
prometheus_port=""

while (($# > 0)); do
  case "$1" in
    --keeper-node-url=*)
      keeper_node_url="${1#*=}"
      shift
      ;;
    --keeper-signer-uri=*)
      keeper_signer_uri="${1#*=}"
      shift
      ;;
    --prometheus-port=*)
      prometheus_port="${1#*=}"
      shift
      ;;
    --prometheus-port)
      if (($# < 2)); then
        echo "keeper-node: --prometheus-port requires a value" >&2
        exit 64
      fi
      prometheus_port="$2"
      shift 2
      ;;
    *)
      # Zombienet supplies --chain, --base-path, --rpc-port, --port and other
      # Substrate-node flags. They have no keeper equivalent.
      shift
      ;;
  esac
done

if [[ "$keeper_node_url" != ws://127.0.0.1:* ]]; then
  echo "keeper-node: a localhost --keeper-node-url is required" >&2
  exit 64
fi
if [[ -z "$keeper_signer_uri" ]]; then
  echo "keeper-node: --keeper-signer-uri is required" >&2
  exit 64
fi
if [[ ! "$prometheus_port" =~ ^[0-9]+$ ]] ||
  ((10#$prometheus_port < 1 || 10#$prometheus_port > 65535)); then
  echo "keeper-node: Zombienet supplied an invalid Prometheus port" >&2
  exit 64
fi

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
keeper_binary="$script_dir/../../keeper/target/release/bleavit-keeper"
if [[ ! -x "$keeper_binary" ]]; then
  echo "keeper-node: release keeper binary is missing or not executable: $keeper_binary" >&2
  exit 69
fi

exec "$keeper_binary" \
  --node-url "$keeper_node_url" \
  --signer-uri "$keeper_signer_uri" \
  --metrics-bind "127.0.0.1:$prometheus_port"
