# Build from an upstream OS or project runtime, never from an old full worker.
ARG BASE_IMAGE=ubuntu:24.04
FROM node:22-bookworm-slim AS agent-runtime
ARG HORIZON_AGENTS=
RUN node -e 'const fs=require("fs");const out="/output/usr/local/bin";fs.mkdirSync(out,{recursive:true});if(process.env.HORIZON_AGENTS){fs.copyFileSync("/usr/local/bin/node",out+"/node");fs.cpSync("/usr/local/lib/node_modules/npm","/output/usr/local/lib/node_modules/npm",{recursive:true});fs.symlinkSync("../lib/node_modules/npm/bin/npm-cli.js",out+"/npm");fs.symlinkSync("../lib/node_modules/npm/bin/npx-cli.js",out+"/npx");}'
FROM node:22-bookworm-slim AS selected
ARG HORIZON_AGENTS=
ARG HORIZON_BROWSERS=
ARG HORIZON_DESKTOP=false
# Only final stripped helpers belong in bin/. Unselected helpers stay in this stage.
COPY bin/ /input/
RUN node -e 'const fs=require("fs"); const out="/output/usr/local/bin"; fs.mkdirSync(out,{recursive:true}); const copy=n=>fs.copyFileSync("/input/"+n,out+"/"+n); copy("horizon-cloud-worker"); if(process.env.HORIZON_BROWSERS)copy("horizon-browser"); if(process.env.HORIZON_DESKTOP==="true")copy("horizon-device"); fs.mkdirSync("/output/etc/horizon-worker",{recursive:true});fs.writeFileSync("/output/etc/horizon-worker/capabilities.json",JSON.stringify({agents:process.env.HORIZON_AGENTS.split(",").filter(Boolean),browsers:process.env.HORIZON_BROWSERS.split(",").filter(Boolean),desktop:process.env.HORIZON_DESKTOP==="true"}));'
FROM ${BASE_IMAGE}
ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    openssh-server tmux git git-lfs gh ca-certificates curl python3 tini util-linux \
    && rm -rf /var/lib/apt/lists/*
COPY --from=agent-runtime /output/ /
ARG HORIZON_AGENTS=
# Separate installation layers make each selected agent's compressed cost measurable.
RUN case ",$HORIZON_AGENTS," in *,codex,*) npm install -g @openai/codex && npm cache clean --force;; esac
RUN case ",$HORIZON_AGENTS," in *,claude,*) npm install -g @anthropic-ai/claude-code && npm cache clean --force;; esac
RUN case ",$HORIZON_AGENTS," in *,grok,*) npm install -g @xai-official/grok && npm cache clean --force;; esac
ARG HORIZON_DESKTOP=false
RUN if [ "$HORIZON_DESKTOP" = true ]; then apt-get update \
    && apt-get install -y --no-install-recommends xvfb openbox x11vnc xauth libxtst6 fonts-dejavu-core \
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
COPY --from=selected /output/ /
COPY horizon-worker-* /usr/local/bin/
RUN chmod 755 /usr/local/bin/horizon-worker-* \
    && ln -s /usr/local/bin/horizon-worker-git-auth /usr/local/bin/gh \
    && mkdir -p /run/sshd /workspace \
    && printf 'AuthorizedKeysFile /run/sshd/horizon-authorized-keys\nPasswordAuthentication no\nPermitRootLogin prohibit-password\nPermitUserEnvironment no\n' > /etc/ssh/sshd_config.d/horizon.conf \
    && horizon-worker-check --git-auth
LABEL io.horizon.worker.contract="1" io.horizon.worker.capabilities="1"
EXPOSE 22
ENTRYPOINT ["/usr/bin/tini", "-s", "--", "/usr/local/bin/horizon-worker-start"]
