# syntax=docker/dockerfile:1

# The "browser machine" image: one browser that answers the browser panels of
# Horizon. Two engines are served:
#
#   chromium  Chromium, headed under Xvfb, with its DevTools endpoint on the
#             loopback of the machine. Horizon reaches it through one SSH forward.
#   firefox   Firefox and geckodriver, headless, for the driver of Horizon that
#             runs inside this container: `docker exec -i -e HOME=/profile
#             <container> /opt/horizon/horizon --browser-host-driver`. The driver
#             keeps the page while no computer is linked, and the profile folder
#             is the mounted one, so a sign-in and a typed form survive both the
#             link and the container.
#
# The binary of the driver is a fifth input of the build: `setup.sh --engine
# firefox --horizon <path>` copies it beside the Dockerfile and the build puts it
# at /opt/horizon/horizon. It must be built for the system and CPU of the
# destination, because that is where it runs.
#
# The driver needs a C library that is not older than the one it was built with,
# and a binary of a current distribution is newer than Debian 13. This image is
# therefore built on Debian 14, whose C library is 2.43: the same one that the
# newest Ubuntu has, so a horizon of such a computer starts here. A binary of a
# newer library still cannot start, so setup.sh reads the library that the
# copied binary needs and compares it with the library of this image before it
# builds anything. Read `README.md` for the whole rule.
#
# Horizon starts its browser headed and passes the arguments itself, so this
# image holds a display (Xvfb) and starts Chromium headed under it. The start
# arguments of Horizon include `--remote-debugging-address=127.0.0.1`, which is
# the loopback of this container. A published Docker port reaches the container
# interface `eth0` and not that loopback, so the entrypoint keeps a `socat`
# bridge from the container interface to the loopback port of Chromium.
#
# Build it on the destination, with the files beside it and, for Firefox, the
# horizon binary as well:
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
# `chromium-browser` of a snap: it needs snapd and a systemd session, and Ubuntu
# has no `firefox-esr` of its own either.
#
# `geckodriver` has no Debian package, so this image takes the release of
# Mozilla with the checksum that Mozilla published for it.
#
# Pin a digest for a long-lived machine: this tag follows Debian 14. Read the
# digest after the first build with `docker image inspect --format '{{index
# .RepoDigests 0}}' horizon-browser-machine:local`.

FROM debian:forky-slim

# The CPU of the image, as BuildKit names it, so geckodriver is the release of
# that CPU. `docker build` sets it.
ARG TARGETARCH

LABEL org.opencontainers.image.title="horizon browser machine" \
      org.opencontainers.image.description="A Chromium DevTools endpoint or a Firefox driver for Horizon browser panels" \
      io.horizon.browser-machine.contract="1"

# ca-certificates the trust store of HTTPS pages; a Recommends of curl, so name it
# chromium        the browser that serves the DevTools endpoint
# curl            the readiness check of the entrypoint and of the HEALTHCHECK
# firefox-esr     the browser of the firefox engine
# fonts-*         Latin (DejaVu, Liberation), CJK (Noto CJK) and colour emoji
# openbox         a small window manager, so a headed window has stable geometry
# socat           the bridge from the container interface to the loopback port
# tini            PID 1, so the browser's children are reaped and signals reach it
# xvfb            the display of the headed browser
RUN DEBIAN_FRONTEND=noninteractive apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install --yes --no-install-recommends \
        ca-certificates \
        chromium \
        curl \
        firefox-esr \
        fonts-dejavu-core \
        fonts-liberation \
        fonts-noto-cjk \
        fonts-noto-color-emoji \
        openbox \
        socat \
        tini \
        xvfb \
    && rm -rf /var/lib/apt/lists/*

# The WebDriver of Firefox. Debian has no package of it, so the image takes the
# release of Mozilla and checks the checksum that Mozilla publishes beside it.
# The version is pinned, so a build of this Dockerfile is the same binary.
ARG GECKODRIVER_VERSION=0.37.1
RUN set -eu; \
    case "${TARGETARCH:-amd64}" in \
        amd64) architecture=linux64 ; sha256=e815130ea95983e162ae91843b48d3a3ce991735635fce83a647afde21e09f7e ;; \
        arm64) architecture=linux-aarch64 ; sha256=8fd90b951422fbad5b56539fb344dff66eb3f986d615d8d6fde9f1f62dad610c ;; \
        *) echo "no geckodriver release for the CPU $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    release="geckodriver-v${GECKODRIVER_VERSION}-${architecture}.tar.gz"; \
    curl --fail --location --silent --show-error --output "/tmp/$release" \
        "https://github.com/mozilla/geckodriver/releases/download/v${GECKODRIVER_VERSION}/$release"; \
    echo "$sha256  /tmp/$release" | sha256sum --check --status; \
    tar --extract --gzip --file "/tmp/$release" --directory /usr/local/bin; \
    chmod 0755 /usr/local/bin/geckodriver; \
    rm -f "/tmp/$release"

COPY entrypoint.sh /usr/local/bin/browser-machine-entrypoint
RUN chmod 0755 /usr/local/bin/browser-machine-entrypoint

# The driver of the firefox engine: the same horizon binary as the computer of
# the person, built for this system and CPU. An empty placeholder keeps a
# Chromium build working without it.
COPY horizon /opt/horizon/horizon
RUN chmod 0755 /opt/horizon/horizon \
    && /opt/horizon/horizon --version >/dev/null 2>&1 || true

# The profile folder is a mount of the profile folder of the destination, so a
# sign-in survives `docker stop` and `docker start`.
ENV BM_PROFILE_DIR=/profile \
    BM_ENGINE=chromium \
    BM_DISPLAY=99 \
    BM_SCREEN=1920x1080x24 \
    BM_WINDOW_SIZE=1600,900 \
    BM_CDP_PORT=9223 \
    BM_BRIDGE_PORT=9222

VOLUME ["/profile"]
EXPOSE 9222

# The endpoint answers only when Xvfb, the bridge and Chromium all run. The
# firefox engine answers no port: its health is the container, and the driver of
# Horizon reports its own failures in the panel.
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
    CMD ["sh", "-c", "if [ \"$BM_ENGINE\" = firefox ]; then exit 0; fi; curl --fail --silent --show-error --max-time 3 --output /dev/null http://127.0.0.1:9222/json/version"]

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/browser-machine-entrypoint"]
