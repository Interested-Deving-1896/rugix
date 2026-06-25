#!/usr/bin/env bash

set -euo pipefail

pushd crates/apps/rugix-ctrl
./generate-json-schema.sh
popd

pushd crates/libs/rugix-bundle
./generate-json-schema.sh
popd

cargo +nightly fmt
