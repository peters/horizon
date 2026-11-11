# syntax=docker/dockerfile:1

# The "browser machine" image: one Chromium that answers a DevTools endpoint on
# the loopback of the machine, for the browser panels of Horizon.
#
# Horizon starts its browser headed and passes the arguments itself, so this
# image holds a display (Xvfb) and starts Chromium headed under it. The start
# arguments of Horizon include `--remote-debugging-address=127.0.0.1`, which is
# the loopback of this container. A published Docker port reaches the container
# interface `eth0` and not that loopback, so the entrypoint keeps a `socat`
# bridge from the container interface to the loopback port of Chromium.
#
# Build it on the destination, with the two files beside it:
#
#   docker build -t horizon-browser-machine:local .
#
# Run it with `setup.sh`, never by hand: it publishes one port on the loopback
# of the Docker host and mounts the profile folder.
#
# Chromium runs as root with `--no-sandbox`, because Docker's default seccomp
# profile refuses the user namespaces that the Chromium sandbox needs. The page
# sandbox is therefore off. The trust boundary of this choice is the loopback
# publish plus the SSH tunnel of Horizon: every local account of the machine can
# reach the DevTools port of this container.
#
# Debian's `chromium` is a native Debian package. Do not use the Ubuntu
# `chromium-browser` of a snap: it needs snapd and a systemd session.
#
# Pin a digest for a long-lived machine: this tag follows Debian 13. Read the
# digest after the first build with `docker image inspect --format '{{index
# .RepoDigests 0}}' horizon-browser-machine:local`.

FROM debian:13-slim

LABEL org.opencontainers.image.title="horizon browser machine" \
      org.opencontainers.image.description="Chromium behind a loopback DevTools endpoint for Horizon browser panels" \
      io.horizon.browser-machine.contract="1"

# chromium        the browser that serves the DevTools endpoint
# ca-certificates the trust store of HTTPS pages; a Recommends of curl, so name it
# curl            the readiness check of the entrypoint and of the HEALTHCHECK
# fonts-*         Latin (DejaVu, Liberation), CJK (Noto CJK) and colour emoji
# openbox         a small window manager, so a headed window has stable geometry
# socat           the bridge from the container interface to the loopback port
# tini            PID 1, so Chromium's children are reaped and signals reach it
# xvfb            the display of the headed browser
RUN DEBIAN_FRONTEND=noninteractive apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install --yes --no-install-recommends \
        ca-certificates \
        chromium \
        curl \
        fonts-dejavu-core \
        fonts-liberation \
        fonts-noto-cjk \
        fonts-noto-color-emoji \
        openbox \
        socat \
        tini \
        xvfb \
    && rm -rf /var/lib/apt/lists/*

COPY entrypoint.sh /usr/local/bin/browser-machine-entrypoint
RUN chmod 0755 /usr/local/bin/browser-machine-entrypoint

# The profile folder is a mount of the profile folder of the destination, so a
# sign-in survives `docker stop` and `docker start`.
ENV BM_PROFILE_DIR=/profile \
    BM_DISPLAY=99 \
    BM_SCREEN=1920x1080x24 \
    BM_WINDOW_SIZE=1600,900 \
    BM_CDP_PORT=9223 \
    BM_BRIDGE_PORT=9222

VOLUME ["/profile"]
EXPOSE 9222

# The endpoint answers only when Xvfb, the bridge and Chromium all run.
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
    CMD ["curl", "--fail", "--silent", "--show-error", "--max-time", "3", "--output", "/dev/null", "http://127.0.0.1:9222/json/version"]

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/browser-machine-entrypoint"]
