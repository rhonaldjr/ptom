#!/usr/bin/env bash
# Enforces the Clean Architecture dependency rule (ADR-0002):
#   1 entities  ←  2 use cases  ←  3 adapters  ←  4 apps
# A crate may depend only on workspace crates in the same or an inner layer,
# and layers 1–2 must not depend on runtime/OS/framework crates.
set -euo pipefail

declare -A LAYER=(
  [ptom-rfb]=1 [ptom-codec]=1
  [ptom-security]=2 [ptom-client]=2 [ptom-server]=2
  [ptom-net]=3 [ptom-vault]=3 [ptom-discovery]=3
  [ptom]=4 [ptom-agent]=4 [ptom-passwd]=4
)
FORBIDDEN_IN_CORE='^(tokio|async-std|smol|futures|rustls|tokio-rustls|libcosmic|iced|winit|wgpu|wayland-client|wayland-server|x11rb|smithay|pipewire|ashpd|reis|mdns-sd|keyring|rustix|libc|nix)$'

cd "$(dirname "$0")/.."
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
done < <(cargo metadata --format-version 1 --no-deps |
  jq -r '.packages[] | .name as $p | .dependencies[] | [$p, .name, (.kind // "normal")] | @tsv')

# Members with no dependencies don't appear above; check they have a layer too.
while read -r pkg; do
  [[ -n ${LAYER[$pkg]:-} ]] || { echo "error: crate '$pkg' has no layer"; fail=1; }
done < <(cargo metadata --format-version 1 --no-deps | jq -r '.packages[].name')

((fail == 0)) && echo "layer check passed"
exit "$fail"
