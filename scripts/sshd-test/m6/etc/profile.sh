# M6：交互式登录 shell 里直接敲 claude / codex / opencode / nvtop 也指向假上游与假 GPU。
set -a
. /etc/m6/agents.env
set +a
# iftop 在 /usr/sbin（Debian 给普通用户的 PATH 里没有）。
PATH="$PATH:/usr/sbin"
