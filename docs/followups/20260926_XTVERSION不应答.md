# XTVERSION 不应答，Claude Code 不用同步输出

- 状态：待修
- 记录日期：2026-09-26

## 问题

终端名称与版本查询 XTVERSION（`CSI > q`，应答 `DCS > | 名称 ST`）没有应答。Claude Code 启动时按
XTVERSION、`CSI ?u`、DA1 的顺序查询：XTVERSION 有应答才用 DECRQM 查 2026，支持才每帧包同步输出
（DEC 2026）。所以在 GuoSSHell 里 Claude Code 不用同步输出，一帧的更新可能被拆到两次显示里，
整屏重绘（切换视图、改尺寸）时可能出现画了一半的中间状态。

Codex、opencode 用同步输出不以 XTVERSION 的应答为前提，不受这一点影响。

## 现状

- alacritty_terminal（vte 0.15）不处理 `CSI > q`，rsHell 也没有拦截；DECRQM 2026 正常应答「支持」。
- M6 验收（docs/acceptance-m6-2026-09-26.md §8）中 Claude Code 的场景都通过，但它全程没有包同步输出。

## 下一步

1. rsHell 补丁：引擎收到 `CSI > q`（参数缺省或为 0）时应答 `DCS > | GuoSSHell <版本> ST`，
   版本由宿主传入。
2. 补上之后在 M6 的 agent 用例里确认 Claude Code 开始包同步输出，显示延迟与帧率不变差。
