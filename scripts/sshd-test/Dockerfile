# GuoSSHell 本地验收用的 SSH 服务器：vim / htop / tmux（粘贴、鼠标、主机密钥、私钥认证等验收），
# 以及 M6 的全套（三个 coding agent 与假 AI 上游、全屏 TUI、假 GPU、回环流量、m6-* 辅助程序，
# 见 docs/acceptance-m6-2026-09-26.md）。
# 由 scripts/sshd-test.sh 构建与启动，构建上下文是 scripts/。

# ── nvtop 的假 GPU：NVIDIA mocknvml（Apache-2.0），假的 libnvidia-ml.so ──
FROM golang:1.26-bookworm AS mocknvml
ARG MOCKNVML_REV=f0dde50e4733a83f4151734b35290b4ccad503b8
RUN git clone --filter=blob:none https://github.com/NVIDIA/k8s-test-infra /src \
 && git -C /src checkout --quiet "${MOCKNVML_REV}"
WORKDIR /src/pkg/gpu/mocknvml
RUN make NO_PADDING=1 \
 && mkdir -p /out \
 && cp -a libnvidia-ml.so* /out/ \
 && cp /src/LICENSE /out/LICENSE.mocknvml
COPY sshd-test/m6/etc/nvml-dynamic.awk /tmp/
RUN awk -f /tmp/nvml-dynamic.awk configs/mock-nvml-config-l40s.yaml > /out/nvml.yaml

FROM debian:trixie-slim

RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      openssh-server vim-nox htop tmux less locales python3 ca-certificates \
      procps git ripgrep curl libcap2-bin \
      btop nvtop nload bmon iftop iperf3 \
      nodejs npm \
 && rm -rf /var/lib/apt/lists/*

# 系统默认 locale（SSH 会话经 PAM 读 /etc/default/locale；Docker 的 ENV 进不了 SSH 会话）。
RUN sed -i 's/^# *\(en_US.UTF-8\)/\1/; s/^# *\(zh_CN.UTF-8\)/\1/' /etc/locale.gen \
 && locale-gen \
 && echo 'LANG=en_US.UTF-8' > /etc/default/locale
ENV LANG=en_US.UTF-8

# 登录 probe / probe（与 demo_server 一致）。
RUN useradd -m -s /bin/bash probe \
 && echo 'probe:probe' | chpasswd \
 && mkdir -p /run/sshd /home/probe/.ssh \
 && chmod 700 /home/probe/.ssh \
 && chown -R probe:probe /home/probe/.ssh

# keyboard-interactive 验收：Debian 默认关着；打开后由 PAM 逐项提问（Password:）。
RUN sed -i 's/^#\?KbdInteractiveAuthentication.*/KbdInteractiveAuthentication yes/' /etc/ssh/sshd_config \
 && grep -qx 'KbdInteractiveAuthentication yes' /etc/ssh/sshd_config

# M1 帧率自检脚本：连上后执行 `m1bar`。
COPY m1bar.sh /usr/local/bin/m1bar
RUN chmod 755 /usr/local/bin/m1bar

# ── M6 ──

# 三个 coding agent，版本 pin 在这里（npm 包里按平台带原生二进制）。
ARG CLAUDE_CODE_VERSION=2.1.274
ARG CODEX_VERSION=0.157.1
ARG OPENCODE_VERSION=1.18.32
RUN npm install -g --no-fund --no-audit \
      "@anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}" \
      "@openai/codex@${CODEX_VERSION}" \
      "opencode-ai@${OPENCODE_VERSION}" \
 && npm cache clean --force

# 假 AI 上游：aimock（MIT）+ 我们的剧本，以专用用户运行（entrypoint.sh）。
RUN useradd -r -s /usr/sbin/nologin m6
COPY sshd-test/m6/llm/package.json /opt/m6/llm/
RUN cd /opt/m6/llm && npm install --no-fund --no-audit --omit=dev && npm cache clean --force
COPY sshd-test/m6/llm/*.mjs /opt/m6/llm/

# 假 GPU：库装进系统库路径，配置在 /etc/m6/nvml.yaml（MOCK_NVML_CONFIG 指过去）。
COPY --from=mocknvml /out/libnvidia-ml.so* /usr/local/lib/mocknvml/
COPY --from=mocknvml /out/LICENSE.mocknvml /usr/local/share/doc/mocknvml/LICENSE
COPY --from=mocknvml /out/nvml.yaml /etc/m6/nvml.yaml
RUN echo /usr/local/lib/mocknvml > /etc/ld.so.conf.d/mocknvml.conf && ldconfig

# iftop 抓包要 CAP_NET_RAW：给二进制授权，probe 不用 root 也能跑（Docker 默认带这项能力）。
RUN setcap cap_net_raw+ep /usr/sbin/iftop

# 辅助程序与环境。
COPY sshd-test/m6/bin/ /usr/local/bin/
COPY sshd-test/m6/etc/agents.env /etc/m6/agents.env
COPY sshd-test/m6/etc/profile.sh /etc/profile.d/m6.sh
COPY sshd-test/m6/entrypoint.sh /usr/local/sbin/m6-entrypoint
RUN chmod 755 /usr/local/bin/m6-* /usr/local/sbin/m6-entrypoint

# probe 的家目录：三个 agent 的配置（指向假上游、跳过引导与确认）、btop 的刷新间隔、
# 工作区（git 仓库：Codex 与 Claude Code 按 git 根判定信任）。
COPY --chown=probe:probe sshd-test/m6/home/ /home/probe/
RUN su probe -c 'git config --global user.name probe && git config --global user.email probe@m6.invalid \
      && cd /home/probe/m6 && git init -q && git add -A && git commit -qm m6' \
 && su probe -c "cd /home/probe/.config/opencode && npm install --no-fund --no-audit @opencode-ai/plugin@${OPENCODE_VERSION}" \
 && rm -rf /home/probe/.npm

EXPOSE 22 4010
CMD ["/usr/local/sbin/m6-entrypoint"]
