# syntax=docker/dockerfile:1

# The browser image of issue 1780: a display, a Chromium that the profile host
# drives, and the profile host itself. README.md gives the commands, the
# packages and the limits.
#
# Build it from the repository root, so that the first stage can compile the
# profile host from the sources:
#
#   docker build -f packaging/browser-vm/Dockerfile -t horizon-browser-vm .

FROM rust:1.99.1-bookworm AS profile-host

# rav1e, the video encoder of the browser engine, needs NASM on x86_64.
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install --yes --no-install-recommends nasm \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . /src
RUN cargo build --release --locked -p horizon-browser-cli \
    && install -m 0755 target/release/horizon-browser /usr/local/bin/horizon-browser

FROM debian:12-slim AS browser-vm

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install --yes --no-install-recommends \
        chromium \
        dbus-x11 \
        fonts-dejavu-core \
        fonts-liberation \
        openbox \
        x11vnc \
        xvfb \
    && rm -rf /var/lib/apt/lists/*

COPY --from=profile-host /usr/local/bin/horizon-browser /usr/local/bin/horizon-browser
COPY packaging/browser-vm/entrypoint.sh /usr/local/bin/horizon-browser-vm-start
COPY packaging/browser-vm/host.example.yaml /usr/local/share/horizon-browser-vm/host.example.yaml
RUN chmod 0755 /usr/local/bin/horizon-browser-vm-start

ENV HORIZON_BROWSER_VM_STATE=/state \
    HORIZON_BROWSER_VM_DISPLAY=99 \
    HORIZON_BROWSER_VM_VNC_PORT=5900 \
    HORIZON_BROWSER_VM_SCREEN=1920x1080x24

LABEL org.opencontainers.image.source="https://github.com/peters/horizon" \
    org.opencontainers.image.description="Browser machine for the profile host: display, VNC and Chromium" \
    io.horizon.browser-vm.contract="1"

VOLUME ["/state"]
EXPOSE 5900
ENTRYPOINT ["/usr/local/bin/horizon-browser-vm-start"]
