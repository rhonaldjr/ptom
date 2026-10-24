#!/usr/bin/env bash
# Runs cargo inside ci/builder.Dockerfile, e.g. `scripts/docker-cargo.sh build -p farsight`.
# Uses target-docker/ so it never mixes artifacts with a host build.
set -euo pipefail
cd "$(dirname "$0")/.."
docker build -q -t farsight-builder -f ci/builder.Dockerfile ci >/dev/null
mkdir -p target-docker/.cargo
exec docker run --rm -u "$(id -u):$(id -g)" \
  -e CARGO_HOME=/w/target-docker/.cargo -e CARGO_TARGET_DIR=/w/target-docker \
  -v "$PWD":/w -w /w farsight-builder cargo "$@"
