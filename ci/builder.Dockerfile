# Build environment for the libcosmic viewer, for machines without the GUI
# development libraries. Ubuntu 22.04 (glibc 2.35) so binaries also run on
# Pop!_OS 22.04. Used by scripts/docker-cargo.sh. libpipewire-0.3-dev (0.3.48 on
# 22.04) and libclang-dev (bindgen) are for farsight-capture-wayland; see
# install-spa-headers.sh for why the SPA headers are replaced. libpixman-1-dev is for
# farsight-headless (M7); xwayland and foot (a small terminal) are for its tests.
FROM ubuntu:22.04
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
COPY install-spa-headers.sh /tmp/install-spa-headers.sh
RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      ca-certificates curl build-essential pkg-config cmake git jq xvfb x11-utils x11-xserver-utils bzip2 \
      libxkbcommon-dev libwayland-dev libfontconfig-dev libfreetype-dev libdbus-1-dev libssl-dev \
      libpipewire-0.3-dev libclang-dev libpixman-1-dev xwayland libegl1 foot \
 && rm -rf /var/lib/apt/lists/* \
 && bash /tmp/install-spa-headers.sh && rm /tmp/install-spa-headers.sh \
 && curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal -c rustfmt,clippy \
 && chmod -R a+rwX /usr/local/rustup /usr/local/cargo
