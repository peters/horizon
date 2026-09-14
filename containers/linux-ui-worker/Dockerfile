# Use a registry digest for publication; a local tag permits offline rehearsal.
ARG WORKER_IMAGE=horizon-remote-worker:local
FROM ${WORKER_IMAGE}

RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        libgl1-mesa-dri libvulkan1 libxkbcommon-x11-0 mesa-vulkan-drivers \
        chromium chromium-driver firefox-esr \
        openbox scrot wmctrl x11-utils xauth xdotool xvfb \
    && rm -rf /var/lib/apt/lists/*

# Official Mozilla release assets, checked before extraction (amd64/arm64).
RUN case "$(dpkg --print-architecture)" in \
        amd64) platform=linux64; digest=e815130ea95983e162ae91843b48d3a3ce991735635fce83a647afde21e09f7e ;; \
        arm64) platform=linux-aarch64; digest=8fd90b951422fbad5b56539fb344dff66eb3f986d615d8d6fde9f1f62dad610c ;; \
        *) echo 'unsupported geckodriver architecture' >&2; exit 64 ;; \
    esac \
    && curl --fail --location --max-time 120 --output /tmp/geckodriver.tar.gz \
        "https://github.com/mozilla/geckodriver/releases/download/v0.37.1/geckodriver-v0.37.1-${platform}.tar.gz" \
    && printf '%s  /tmp/geckodriver.tar.gz\n' "${digest}" | sha256sum --check - \
    && tar -xzf /tmp/geckodriver.tar.gz -C /usr/local/bin geckodriver \
    && rm /tmp/geckodriver.tar.gz \
    && chromium --version && chromedriver --version \
    && firefox-esr --version && geckodriver --version

COPY smoke.py /usr/local/bin/horizon-linux-ui-smoke
COPY processes.py /usr/local/bin/processes.py
COPY browser-smoke.py /usr/local/bin/browser-smoke.py
RUN chmod 0755 /usr/local/bin/horizon-linux-ui-smoke
RUN useradd --uid 1000 --create-home --shell /bin/bash horizon-smoke

LABEL io.horizon.worker.linux-ui-smoke="1"
# Preserve the base worker's SSH entrypoint and runtime-only credentials.
