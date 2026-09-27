#!/bin/sh
# 验收服务器的入口：先起 M6 的后台服务（假 AI 上游、回环流量），再前台跑 sshd。
set -e

mkdir -p /tmp/m6
chmod 1777 /tmp/m6
: > /tmp/m6/input.log
chmod 666 /tmp/m6/input.log

# 假 AI 上游（aimock + 剧本），以专用用户运行；日志在 /tmp/m6/llm.log。
su -s /bin/sh m6 -c 'cd /opt/m6/llm && exec node server.mjs >>/tmp/m6/llm.log 2>&1' &

# 三个 agent 各预热一次（首次运行的初始化），等假上游起来之后在后台做。
su probe -c 'sleep 3; exec m6-warmup >>/tmp/m6/warmup.log 2>&1' &

# 回环上的持续流量（网速监控有数可看）：几档速率轮换，曲线才有起伏。
iperf3 -s -D -B 127.0.0.1 --logfile /tmp/m6/iperf3.log
su -s /bin/sh m6 -c '
  while :; do
    for rate in 5M 40M 15M 60M 2M; do
      iperf3 -c 127.0.0.1 -t 5 -b "$rate" >/dev/null 2>&1 || sleep 1
    done
  done' &

# 前台运行、日志进 stderr（docker logs 可见）；SIGHUP 让 sshd 重新读主机密钥。
exec /usr/sbin/sshd -D -e
