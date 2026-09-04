ARG WORKER_BASE_IMAGE=rust:1.95-bookworm@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1

FROM node:24-bookworm-slim@sha256:ba849c60be29959425b8734d57b8b4b7d56f98edd9504c9af091d5281095a71e AS node-runtime

FROM ${WORKER_BASE_IMAGE}

ARG WORKER_BASE_IMAGE
ARG CODEX_VERSION=0.153.2
ARG CLAUDE_VERSION=2.1.260
ARG GEMINI_VERSION=0.58.0
ARG OPENCODE_VERSION=1.18.27
ARG KILO_VERSION=7.5.9
ARG PI_VERSION=0.84.4
ARG GROK_VERSION=1.0.18
ARG GROK_AMD64_SHA256=42f6efb496feafea3e8807b75852cc1f792cba0bd0be8ecbd1c9f7ed5816a7f0
ARG GROK_ARM64_SHA256=e5c96058b6f88b2e9ae7d61401e80ac6bcbcaf1be5681eae19e36a2b8aff121d
ARG TARGETARCH

ENV CARGO_HOME=/usr/local/cargo
ENV RUSTUP_HOME=/usr/local/rustup
ENV RUSTUP_TOOLCHAIN=1.95.0
ENV PATH="/usr/local/cargo/bin:${PATH}"

RUN case "${WORKER_BASE_IMAGE}" in \
        *@sha256:*) base_digest="${WORKER_BASE_IMAGE##*@sha256:}" ;; \
        *) echo "WORKER_BASE_IMAGE must be pinned by sha256 digest" >&2; exit 64 ;; \
    esac \
    && if [ "${#base_digest}" -ne 64 ]; then \
        echo "WORKER_BASE_IMAGE must use a 64-character sha256 digest" >&2; \
        exit 64; \
    fi \
    && case "${base_digest}" in \
        *[!0-9a-f]*) echo "WORKER_BASE_IMAGE sha256 digest must be lowercase hexadecimal" >&2; exit 64 ;; \
    esac

COPY --from=node-runtime /usr/local/bin/node /usr/local/bin/node
COPY --from=node-runtime /usr/local/include/node/ /usr/local/include/node/
COPY --from=node-runtime /usr/local/lib/node_modules/ /usr/local/lib/node_modules/

RUN ln -sfn node /usr/local/bin/nodejs \
    && ln -sfn ../lib/node_modules/corepack/dist/corepack.js /usr/local/bin/corepack \
    && ln -sfn ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
    && ln -sfn ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx

RUN apt-get update \
    && apt-get install --yes --no-install-recommends \
        build-essential \
        ca-certificates \
        clang \
        cmake \
        curl \
        gh \
        git \
        git-lfs \
        libasound2-dev \
        libgl-dev \
        libvulkan-dev \
        libwayland-dev \
        libxcb-render0-dev \
        libxcb-shape0-dev \
        libxcb-xfixes0-dev \
        libxkbcommon-dev \
        openssh-server \
        pkg-config \
        python3 \
        rsync \
        tar \
        tmux \
    && rm -rf /var/lib/apt/lists/*

RUN rustup toolchain install 1.95.0 --profile minimal \
    && rustup component add --toolchain 1.95.0 rustfmt clippy

# Keep this explicit: npm must fail if a new dependency adds an unreviewed
# lifecycle script instead of silently skipping or running it.
RUN npm install --global --no-audit --no-fund --strict-allow-scripts \
        --allow-scripts="@anthropic-ai/claude-code,@kilocode/cli,opencode-ai,@github/keytar,node-pty,@google/genai,protobufjs" \
        "@openai/codex@${CODEX_VERSION}" \
        "@anthropic-ai/claude-code@${CLAUDE_VERSION}" \
        "@google/gemini-cli@${GEMINI_VERSION}" \
        "opencode-ai@${OPENCODE_VERSION}" \
        "@kilocode/cli@${KILO_VERSION}" \
        "@earendil-works/pi-coding-agent@${PI_VERSION}" \
    && npm cache clean --force

RUN case "${TARGETARCH}" in \
        amd64) grok_arch=x86_64; grok_sha256="${GROK_AMD64_SHA256}" ;; \
        arm64) grok_arch=aarch64; grok_sha256="${GROK_ARM64_SHA256}" ;; \
        *) echo "Unsupported Grok target architecture: ${TARGETARCH}" >&2; exit 64 ;; \
    esac \
    && curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location \
        "https://x.ai/cli/grok-${GROK_VERSION}-linux-${grok_arch}" \
        --output /usr/local/bin/grok \
    && printf '%s  %s\n' "${grok_sha256}" /usr/local/bin/grok | sha256sum --check --strict - \
    && chmod 0755 /usr/local/bin/grok

RUN node --version \
    && npm --version \
    && codex --version \
    && claude --version \
    && gemini --version \
    && opencode --version \
    && kilo --version \
    && pi --version \
    && grok --version \
    && rustc --version \
    && cargo --version \
    && git --version \
    && git lfs version \
    && gh --version \
    && rsync --version \
    && tar --version \
    && tmux -V

WORKDIR /opt/horizon-dependency-cache
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN mkdir -p \
        crates/horizon-browser/src \
        crates/horizon-browser-cli/src \
        crates/horizon-browser-mcp/src \
        crates/horizon-browser-protocol/src \
        crates/horizon-core/src \
        crates/horizon-cursor/src \
        crates/horizon-ui/src \
    && touch \
        crates/horizon-browser/src/lib.rs \
        crates/horizon-browser-cli/src/lib.rs \
        crates/horizon-browser-cli/src/main.rs \
        crates/horizon-browser-mcp/src/lib.rs \
        crates/horizon-browser-mcp/src/main.rs \
        crates/horizon-browser-protocol/src/lib.rs \
        crates/horizon-core/src/lib.rs \
        crates/horizon-cursor/src/lib.rs \
        crates/horizon-ui/src/main.rs \
    && cargo fetch --locked \
    && rm -rf /opt/horizon-dependency-cache

WORKDIR /workspace/horizon

COPY containers/remote-worker/entrypoint.sh /usr/local/bin/horizon-worker-entrypoint
COPY containers/remote-worker/session.sh /usr/local/bin/horizon-agent-session
COPY containers/remote-worker/rust-path.sh /etc/profile.d/horizon-rust.sh
COPY containers/remote-worker/sshd_config /etc/ssh/sshd_config.d/99-horizon-worker.conf

RUN chmod 0755 /usr/local/bin/horizon-worker-entrypoint /usr/local/bin/horizon-agent-session \
    && chmod 0644 /etc/profile.d/horizon-rust.sh /etc/ssh/sshd_config.d/99-horizon-worker.conf \
    && git lfs install --system \
    && rm -f /etc/ssh/ssh_host_* \
    && rm -rf \
        /root/.cache \
        /root/.cargo \
        /root/.claude \
        /root/.claude.json \
        /root/.codex \
        /root/.config \
        /root/.docker \
        /root/.gemini \
        /root/.gitconfig \
        /root/.grok \
        /root/.kilo \
        /root/.local \
        /root/.netrc \
        /root/.npm \
        /root/.npmrc \
        /root/.opencode \
        /root/.pi \
        /root/.rustup \
        /root/.ssh \
    && mkdir -p /run/sshd /workspace/horizon

ARG WORKER_IMAGE_VERSION=0.1.0
RUN if [ -z "${WORKER_IMAGE_VERSION}" ]; then \
        echo "WORKER_IMAGE_VERSION must not be empty" >&2; exit 64; \
    fi
LABEL org.opencontainers.image.title="Horizon remote worker" \
      org.opencontainers.image.description="Provider-neutral interactive coding worker for Horizon" \
      org.opencontainers.image.version="${WORKER_IMAGE_VERSION}"
ENV HORIZON_WORKER_IMAGE_VERSION=${WORKER_IMAGE_VERSION}

EXPOSE 22
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/horizon-worker-entrypoint"]
