# A TigerVNC server for Farsight interop tests (black-box use only; ADR-0011).
# BASE=debian:trixie-slim gives a TigerVNC with RSA-AES support.
ARG BASE=debian:bookworm-slim
FROM ${BASE}
RUN apt-get update \
 && apt-get install -y --no-install-recommends tigervnc-standalone-server tigervnc-tools x11-xserver-utils openssl \
 && rm -rf /var/lib/apt/lists/*
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
EXPOSE 5901
ENTRYPOINT ["/entrypoint.sh"]
