# bracketed paste 未实现（上游缺 display_modes 字段）

- 状态：**已解决**（2026-09-25）
- 记录日期：2026-09-23

## 现状

M2a 的粘贴是直发原文（当作键盘输入）。多行粘贴在 shell 里会逐行执行、
在 vim autoindent 下会缩进阶梯。要按远端模式正确包裹（DECSET 2004
的 `\e[200~ … \e[201~`），必须知道远端有没有开启括号粘贴模式：
alacritty 引擎内部有这个状态，但上游 rsHell 的
`TerminalDisplayModes` 没有暴露（只有 mouse_reporting / alternate_screen 等）。

## 下一步

按 PLAN §9 的 fork 规则改上游（不是无限期绕开）：
adapter 把 `TermMode::BRACKETED_PASTE` 映射成 `TerminalDisplayModes`
的新字段 `bracketed_paste`；我们读该字段，开启时粘贴包
`\e[200~/\e[201~`，未开启直发原文。fork 后在 `rust/UPSTREAM.md`
记录改了哪几行、为什么。

## 结论

rsHell fork 补丁 P1 暴露了 `TerminalDisplayModes.bracketed_paste`（见 `rust/UPSTREAM.md`）。
粘贴与键入共用 `InputRequest`（粘贴标记 `paste = true`）以保留顺序，由 Rust 处理：换行统一成 CR、剔除 Tab 以外的控制字符（防 `\e[201~`
注入），远端开启 DECSET 2004 时包 `\e[200~ … \e[201~`，未开启直发。
