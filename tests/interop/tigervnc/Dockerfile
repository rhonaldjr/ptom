# A TigerVNC server for Farsight interop tests (black-box use only; ADR-0011).
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends tigervnc-standalone-server tigervnc-tools x11-xserver-utils \
 && rm -rf /var/lib/apt/lists/*
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
EXPOSE 5901
ENTRYPOINT ["/entrypoint.sh"]
