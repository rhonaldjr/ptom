#!/usr/bin/env bash
# Enforces the Clean Architecture dependency rule (ADR-0002):
#   1 entities  ←  2 use cases  ←  3 adapters  ←  4 apps
# A crate may depend only on workspace crates in the same or an inner layer,
# and layers 1–2 must not depend on runtime/OS/framework crates.
set -euo pipefail

declare -A LAYER=(
  [farsight-rfb]=1 [farsight-codec]=1
  [farsight-security]=2 [farsight-client]=2 [farsight-server]=2
  [farsight-net]=3 [farsight-vault]=3 [farsight-discovery]=3 [farsight-desktop]=3
  [farsight]=4 [farsight-agent]=4 [farsight-passwd]=4
)
FORBIDDEN_IN_CORE='^(tokio|async-std|smol|futures|rustls|tokio-rustls|libcosmic|iced|winit|wgpu|wayland-client|wayland-server|x11rb|smithay|pipewire|ashpd|reis|mdns-sd|keyring|rustix|libc|nix)$'

cd "$(dirname "$0")/.."
# Fail loudly if metadata can't be read (e.g. cargo missing): an empty
# dependency list must never count as "passed".
metadata=$(cargo metadata --format-version 1 --no-deps) || { echo "error: cargo metadata failed"; exit 2; }
fail=0
while IFS=$'\t' read -r pkg dep kind; do
  pkg_layer=${LAYER[$pkg]:-}
  if [[ -z $pkg_layer ]]; then
    echo "error: crate '$pkg' has no layer; add it to LAYER in scripts/check-layers.sh"
    fail=1
    continue
  fi
  [[ $kind == dev ]] && continue
  dep_layer=${LAYER[$dep]:-}
  if [[ -n $dep_layer ]] && ((dep_layer > pkg_layer)); then
    echo "error: $pkg (layer $pkg_layer) depends on outer-layer crate $dep (layer $dep_layer)"
    fail=1
  elif [[ -z $dep_layer ]] && ((pkg_layer <= 2)) && [[ $dep =~ $FORBIDDEN_IN_CORE ]]; then
    echo "error: $pkg (layer $pkg_layer) must not depend on runtime/OS/framework crate $dep"
    fail=1
  fi
done < <(jq -r '.packages[] | .name as $p | .dependencies[] | [$p, .name, (.kind // "normal")] | @tsv' <<<"$metadata")

# Members with no dependencies don't appear above; check they have a layer too.
while read -r pkg; do
  [[ -n ${LAYER[$pkg]:-} ]] || { echo "error: crate '$pkg' has no layer"; fail=1; }
done < <(jq -r '.packages[].name' <<<"$metadata")

((fail == 0)) && echo "layer check passed"
exit "$fail"
