#!/usr/bin/env bash
set -euxo pipefail

cd "$(git -C "$(dirname "$0")" rev-parse --show-toplevel)/rust-mqtt-adapter"

cargo test
cargo test -- --ignored
