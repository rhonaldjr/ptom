#!/usr/bin/env bash
# Runs cargo inside ci/builder.Dockerfile, e.g. `scripts/docker-cargo.sh build -p farsight`.
# Uses target-docker/ so it never mixes artifacts with a host build.
set -euo pipefail
cd "$(dirname "$0")/.."
docker build -q -t farsight-builder -f ci/builder.Dockerfile ci >/dev/null
mkdir -p target-docker/.cargo
# /etc/passwd lets programs (e.g. Xwayland's access check) resolve the user.
exec docker run --rm -u "$(id -u):$(id -g)" -v /etc/passwd:/etc/passwd:ro -v /etc/group:/etc/group:ro \
  -e XDG_RUNTIME_DIR=/tmp \
  -e CARGO_HOME=/w/target-docker/.cargo -e CARGO_TARGET_DIR=/w/target-docker \
  -v "$PWD":/w -w /w farsight-builder cargo "$@"
