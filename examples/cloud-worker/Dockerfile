# Build from an upstream OS or project runtime, never from an old full worker.
ARG BASE_IMAGE=ubuntu:24.04
FROM node:22-bookworm-slim AS agent-runtime
ARG HORIZON_AGENTS=
RUN node -e 'const fs=require("fs");const out="/output/usr/local/bin";fs.mkdirSync(out,{recursive:true});if(process.env.HORIZON_AGENTS){fs.copyFileSync("/usr/local/bin/node",out+"/node");fs.cpSync("/usr/local/lib/node_modules/npm","/output/usr/local/lib/node_modules/npm",{recursive:true});fs.symlinkSync("../lib/node_modules/npm/bin/npm-cli.js",out+"/npm");fs.symlinkSync("../lib/node_modules/npm/bin/npx-cli.js",out+"/npx");}'
FROM node:22-bookworm-slim AS selected
ARG HORIZON_AGENTS=
ARG HORIZON_BROWSERS=
ARG HORIZON_DESKTOP=false
ARG HORIZON_BROWSERSTACK=false
# Only final stripped helpers belong in bin/. Unselected helpers stay in this stage.
COPY bin/horizon-cloud-worker bin/horizon-browser bin/horizon-device /input/
RUN node -e 'const fs=require("fs"); const out="/output/usr/local/bin"; fs.mkdirSync(out,{recursive:true}); const copy=n=>fs.copyFileSync("/input/"+n,out+"/"+n); copy("horizon-cloud-worker"); if(process.env.HORIZON_BROWSERS || process.env.HORIZON_BROWSERSTACK==="true")copy("horizon-browser"); if(process.env.HORIZON_DESKTOP==="true")copy("horizon-device"); fs.mkdirSync("/output/etc/horizon-worker",{recursive:true});fs.writeFileSync("/output/etc/horizon-worker/capabilities.json",JSON.stringify({agents:process.env.HORIZON_AGENTS.split(",").filter(Boolean),browsers:process.env.HORIZON_BROWSERS.split(",").filter(Boolean),desktop:process.env.HORIZON_DESKTOP==="true",...(process.env.HORIZON_BROWSERSTACK==="true"?{browserstack:{targets:[],local_ports:[]}}:{})}));'
FROM ${BASE_IMAGE}
ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    openssh-server tmux git git-lfs gh rsync ca-certificates curl python3 tini util-linux \
    && rm -rf /var/lib/apt/lists/*
COPY --from=agent-runtime /output/ /
ARG HORIZON_DESKTOP=false
RUN if [ "$HORIZON_DESKTOP" = true ]; then apt-get update \
    && apt-get install -y --no-install-recommends xvfb openbox x11vnc xauth x11-utils libxtst6 fonts-dejavu-core \
    && rm -rf /var/lib/apt/lists/*; fi
ARG HORIZON_BROWSERS=
RUN case ",$HORIZON_BROWSERS," in *,chromium,*) \
    curl -fsSL https://dl.google.com/linux/linux_signing_key.pub -o /etc/apt/trusted.gpg.d/google.asc \
    && echo 'deb [arch=amd64] https://dl.google.com/linux/chrome/deb/ stable main' > /etc/apt/sources.list.d/chrome.list \
    && apt-get update && apt-get install -y --no-install-recommends google-chrome-stable \
    && rm -rf /var/lib/apt/lists/*;; esac
RUN case ",$HORIZON_BROWSERS," in *,firefox,*) \
    mkdir -p /etc/apt/keyrings \
    && curl -fsSL https://packages.mozilla.org/apt/repo-signing-key.gpg -o /etc/apt/keyrings/mozilla.asc \
    && echo 'deb [signed-by=/etc/apt/keyrings/mozilla.asc] https://packages.mozilla.org/apt mozilla main' > /etc/apt/sources.list.d/mozilla.list \
    && printf 'Package: firefox*\nPin: origin packages.mozilla.org\nPin-Priority: 1000\n' > /etc/apt/preferences.d/mozilla \
    && apt-get update && apt-get install -y --no-install-recommends firefox \
    && curl -fsSL https://github.com/mozilla/geckodriver/releases/download/v0.36.0/geckodriver-v0.36.0-linux64.tar.gz -o /tmp/geckodriver.tar.gz \
    && echo '0bde38707eb0a686a20c6bd50f4adcc7d60d4f73c60eb83ee9e0db8f65823e04  /tmp/geckodriver.tar.gz' | sha256sum -c - \
    && tar -xzf /tmp/geckodriver.tar.gz -C /usr/local/bin \
    && rm -f /tmp/geckodriver.tar.gz && rm -rf /var/lib/apt/lists/*;; esac
ARG HORIZON_BROWSERSTACK=false
RUN if [ "$HORIZON_BROWSERSTACK" = true ]; then \
    curl -fsSL https://www.browserstack.com/browserstack-local/BrowserStackLocal-linux-x64.zip -o /tmp/browserstack.zip \
    && echo '0b2349bde6a33fc417597a7960903fb7a80188ea738aa3e675e2eaae07ad7442  /tmp/browserstack.zip' | sha256sum -c - \
    && python3 -c 'import zipfile; zipfile.ZipFile("/tmp/browserstack.zip").extract("BrowserStackLocal", "/usr/local/bin")' \
    && chmod 755 /usr/local/bin/BrowserStackLocal && rm /tmp/browserstack.zip; fi
ARG HORIZON_AGENTS=
# Separate installation layers make each selected agent's compressed cost measurable.
# They follow the heavier layers, and each is keyed on the release Horizon passes,
# so a newer release rebuilds only that agent and the layers after it. An empty
# version installs the current latest release, which the layer cache then keeps.
ARG HORIZON_GROK_VERSION=
RUN case ",$HORIZON_AGENTS," in *,grok,*) npm install -g "@xai-official/grok@${HORIZON_GROK_VERSION:-latest}" && npm cache clean --force;; esac
ARG HORIZON_CODEX_VERSION=
RUN case ",$HORIZON_AGENTS," in *,codex,*) npm install -g "@openai/codex@${HORIZON_CODEX_VERSION:-latest}" && npm cache clean --force;; esac
ARG HORIZON_CLAUDE_VERSION=
RUN case ",$HORIZON_AGENTS," in *,claude,*) npm install -g "@anthropic-ai/claude-code@${HORIZON_CLAUDE_VERSION:-latest}" && npm cache clean --force;; esac
# horizon-worker-check requires each selected agent to report its recorded version.
RUN mkdir -p /etc/horizon-worker && python3 -c 'import json, pathlib; root = pathlib.Path("/usr/local/lib/node_modules"); packages = {"codex": "@openai/codex", "claude": "@anthropic-ai/claude-code", "grok": "@xai-official/grok"}; versions = {agent: json.loads((root / name / "package.json").read_text())["version"] for agent, name in packages.items() if (root / name / "package.json").is_file()}; pathlib.Path("/etc/horizon-worker/agent-versions.json").write_text(json.dumps(versions, sort_keys=True) + "\n")'
COPY --from=selected /output/ /
COPY horizon-worker-start \
    horizon-worker-check \
    horizon-worker-configure \
    horizon-worker-supervise \
    horizon-worker-idle \
    horizon-worker-stop \
    horizon-worker-run \
    horizon-worker-session \
    horizon-worker-source \
    horizon-worker-import \
    horizon-worker-siblings \
    horizon-worker-git-auth \
    horizon-worker-browserstack /usr/local/bin/
RUN chmod 755 /usr/local/bin/horizon-worker-* \
    && ln -s /usr/local/bin/horizon-worker-git-auth /usr/local/bin/gh \
    && mkdir -p /run/sshd /workspace \
    && printf 'AuthorizedKeysFile /run/sshd/horizon-authorized-keys\nPasswordAuthentication no\nPermitRootLogin prohibit-password\nPermitUserEnvironment no\n' > /etc/ssh/sshd_config.d/horizon.conf \
    && horizon-worker-check --git-auth
LABEL io.horizon.worker.contract="1" io.horizon.worker.capabilities="1"
EXPOSE 22
ENTRYPOINT ["/usr/bin/tini", "-s", "--", "/usr/local/bin/horizon-worker-start"]
