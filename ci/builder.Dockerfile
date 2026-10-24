# Build environment for the libcosmic viewer, for machines without the GUI
# development libraries. Ubuntu 22.04 (glibc 2.35) so binaries also run on
# Pop!_OS 22.04. Used by scripts/docker-cargo.sh.
FROM ubuntu:22.04
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      ca-certificates curl build-essential pkg-config cmake git jq \
      libxkbcommon-dev libwayland-dev libfontconfig-dev libfreetype-dev libdbus-1-dev libssl-dev \
 && rm -rf /var/lib/apt/lists/* \
 && curl -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal -c rustfmt,clippy \
 && chmod -R a+rwX /usr/local/rustup /usr/local/cargo
