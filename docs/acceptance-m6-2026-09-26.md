# M6 验收计划：coding agent、全屏 TUI 与设备矩阵（2026-09-26）

> M6 的验收定义：要回答的问题、测试台、用例、度量与通过标准、真机清单，以及执行结果。
> PLAN.md §5 M6 只放摘要，细节以本文件为准。执行结果回填到 §8。

## 1. 要回答的问题

1. **coding agent**：Claude Code、Codex CLI、opencode 在 GuoSSHell 里运行时——模型高速流式输出、
   连续工具调用、多个 subagent 并发且在它们之间切换视图——App 有没有**性能问题**（帧率下降、显示
   落后于输出、打字不跟手、内存上涨）和**显示 bug**（字符丢失或错位、残影、闪烁、滚回被刷乱、画面卡住）。
2. **全屏 TUI**：btop、nvtop、网速监控（nload、bmon、iftop）进入全屏（备用屏）、全屏中改尺寸、
   退出全屏时有没有问题。
3. **设备**：iPhone 17 Pro Max、iPhone 17、iPad Pro 13 英寸、iPad Pro 11 英寸上运行正常；
   iPad 上硬件键盘与鼠标 / 触控板事件正常。

## 2. 测试台

### 2.1 验收服务器

沿用 `scripts/sshd-test.sh`（Docker，登录 probe / probe），镜像扩展为 M6 全套，基础镜像换成
Debian trixie（btop 1.3.2、nvtop 3.2.0 在 main 里）：

| 内容 | 说明 |
|---|---|
| 三个 agent | Claude Code、Codex CLI、opencode，版本 pin 在 Dockerfile。配置预置在 probe 的家目录：全部指向容器内的假上游；跳过登录、引导、信任目录与权限确认；关闭自动更新、遥测与后台下载 |
| 假 AI 上游 | 容器内常驻（§2.2）；它的端口只映射到本机回环，测试从这里读请求日志、剧本决定与回显记录 |
| 全屏 TUI | btop、nvtop（配假 GPU，§2.4）、nload、bmon、iftop（`cap_net_raw`，非 root 可用）、htop、vim |
| 持续流量 | 容器内 iperf3 在回环上持续收发，网速监控有数可看 |
| 辅助程序 | `/usr/local/bin/m6-*`（§2.3） |

容器随 `sshd-test.sh up` 起 sshd、假上游与流量，并在后台把三个 agent 各预热一次（首次运行的初始化，
opencode 首次启动要装插件、建数据库）。假上游同时提供请求日志、最近的剧本决定（测试据此判断 agent
推进到了哪一步）与回显程序记下的输入。

### 2.2 假 AI 上游

**复用 [aimock](https://github.com/CopilotKit/aimock)（MIT，Node，无依赖）**，版本 pin 在 Dockerfile。
它同时实现三种协议的流式应答，含工具调用与思考块：

| agent | 协议 |
|---|---|
| Claude Code | Anthropic Messages（SSE） |
| Codex CLI | OpenAI Responses（SSE；Codex 已不支持 Chat Completions） |
| opencode | OpenAI Chat Completions（SSE，经 `@ai-sdk/openai-compatible`） |

我们只写一层剧本（Node 脚本，用 aimock 的编程接口：请求 → 按剧本生成回复），外加一层很薄的转发，
补 aimock 不认识的两处 Codex 协议扩展：

- Codex 的 subagent 工具声明在命名空间里（`collaboration`），调用必须带上同一个命名空间，aimock 生成的
  工具调用不带——转发层按请求里的声明给流补上；
- agent 之间的消息是 `agent_message` 输入项，aimock 会丢掉——转发层把它改写成普通的用户消息，
  剧本才看得到子任务与汇报。

剧本的规则：

- **选场景**：用户第一句话里的标记 `[m6:<场景>]`；subagent 由派发时写进它 prompt 的标记
  `[m6-sub:<场景>:<序号>]` 识别。
- **定轮次**：剧本发出的每个工具调用的 id 都带「场景 / 轮次」，下一次请求里最近的工具结果指向哪一轮，
  就回下一轮——回复只由请求内容决定，并发与重试都安全，不需要会话状态。
- **工具意图按 agent 翻译**：执行命令、读文件、写文件、改文件、派 subagent、更新待办，映射到各 agent
  自己的工具名与参数（请求里带的工具列表为准）。
- **速率**：按剧本分档（每档是 aimock 的每秒块数 × 每块字符数），首 token 延迟可配。
- **后台 subagent**：Claude Code 的 subagent 在后台运行——派发后立刻得到「已启动」的工具结果，
  每个完成时再以一条用户消息通知主 agent；主 agent 的剧本按收到的完成通知数决定何时总结。
- **旁路请求**：agent 自己发的会话标题、摘要等请求（不带工具），回短文本或它要的 JSON，不进剧本。
- **确定性**：文本由固定种子生成，同一场景每次输出相同，截图与统计可以前后对比。

剧本（每个场景的最后一条消息以 `M6-DONE <场景>` 结尾，供自动化判断结束）：

| 场景 | 主 agent | subagent |
|---|---|---|
| `stream` | 一轮：思考约 300 token + 正文约 6000 token（标题、列表、代码块、表格、中英混排），约 2000 token/s | — |
| `burst` | 同上，约 8000 token/s | — |
| `tools` | 6 轮：执行命令、读文件、写文件、改文件（diff）、1500 行输出的命令、再执行；每轮先有一段流式文字；最后约 1200 token 总结 | — |
| `subagents` | 一轮派 6 个 subagent → 等结果 → 约 1500 token 总结（约 8000 token/s） | 各自 2 轮工具调用 + 约 2000 token 总结（每个约 250 token/s，6 个并发、持续约 8 秒） |
| `long` | 30 轮，每轮约 400 token（中英交替）+ 一次输出 100 行的命令；最后约 800 token 总结 | — |
| `cjk` | 中文为主的 markdown（长段落自动折行、表格、代码注释） | — |

emoji 只放在 Claude Code 与 opencode 的剧本里：aimock 按 UTF-16 分块会把 emoji 切成两半，
Codex 的解析器会丢掉那一块。

### 2.3 辅助程序（容器内）

| 命令 | 作用 |
|---|---|
| `m6-agent <claude\|codex\|opencode> <场景> [--inline]` | 在一个干净的工作目录里以场景标记为开场白启动 agent 的交互界面；`--inline` 为行内模式 |
| `m6-tui <程序>` | 先打印标记行，再启动 TUI；退出后打印标记行并查询终端模式（见 `m6-probe`） |
| `m6-probe` | 用 DECRQM / CPR / DA 查询终端当前的模式与光标（TUI 退出后查残留） |
| `m6-keyecho [模式…]` | 原始模式下按要求打开鼠标上报 / bracketed paste / 应用光标键，记录收到的每个字节（屏幕 + 日志） |
| `m6-sync <情形>` | 同步输出（DEC 2026）：成对、BSU 后停住、BSU 后被杀 |
| `m6-cjk` | 宽字符排布图样：整行中文、宽字符后的着色段、emoji、组合字符、框线 |
| `m6-flood <每秒行数> <秒数> [cjk]` | 可控速率的彩色 / 中文洪水输出 |
| `m6-warmup` | 容器起来后把三个 agent 各非交互地跑一次，做完首次运行的初始化 |

回显程序只记录原始字节，断言直接对照期望的字节序列（xterm 规范），不在容器里做解码。

### 2.4 nvtop 的假 GPU

nvtop 没有 GPU 时打印「No GPU to monitor.」直接退出。**复用 NVIDIA 的
[mocknvml](https://github.com/NVIDIA/k8s-test-infra/tree/main/pkg/gpu/mocknvml)**（Apache-2.0）：
假的 `libnvidia-ml.so`，Dockerfile 里多阶段构建（Go + cgo），GPU 数量与型号由它的 YAML 配置给出；
不配置进程列表（配了会让 nvtop 崩溃）。nvtop 的显示逻辑与真实 GPU 相同。

### 2.5 App 侧度量与自检

| 项 | 来源 | 说明 |
|---|---|---|
| 出帧率、render / pack 耗时、帧字节 | Rust `PerfStats`（已有，5 秒一窗） | 画面一致性自检的请求顺带交出当前窗口，测试据此按段统计 |
| **显示延迟** | Rust `PerfStats`（新增） | 远端字节进引擎 → 含它的帧被 Dart 确认，p50 / p95 / max；帧流控下 Dart 跟不上会表现为它变大 |
| ACK 超时次数、远端输出吞吐 | Rust `PerfStats`（新增） | ACK 超时 = Dart 250 ms 内没处理完一帧 |
| 帧应用耗时 | Dart | 解码 + 填行池 |
| Flutter 帧 build / raster、卡顿帧 | Dart（`FrameTiming`） | 卡顿 = 超过一个刷新周期 |
| 内存 | Dart | 进程 RSS |
| **画面一致性** | Rust + Dart（新增，测试用） | Rust 给出最后发出的那一帧（逐列）及其序号，Dart 画完同一帧后与自己的行池逐列比对——字符丢失、错位、帧没画上都会被抓到；持续刷新的 TUI 也能比 |

度量一窗一行 JSON 打进日志（`[m6-perf] {…}`），debug 与 profile 构建都有；集成测试直接订阅，写进报告。

### 2.6 自动化

| 层 | 工具 | 覆盖 |
|---|---|---|
| App 内 | Flutter `integration_test`（`flutter drive`；iOS 模拟器 + macOS） | 连验收服务器，跑 A、B、C、E 的场景，以及 D 类里 XCUITest 合成不了的部分（注入按键与悬停）；读 App 自己的终端缓冲断言；画面一致性；截图；度量汇总成 JSON |
| 系统输入 | XCUITest（`ios/RunnerUITests`） | iPad：系统合成的硬件键盘（`typeKey` + 修饰键）与指针事件（点按、右键、拖动、滚动），走 UIKit → Flutter 引擎的真实路径（D 类）；各设备：真实旋转下的全屏 TUI（截图） |
| 编排 | `scripts/m6.sh` | 起服务器、建 / 起四台模拟器、按矩阵跑、汇总报告 |

模拟器上 Flutter 只有 debug 构建（profile / release 不支持模拟器），所以**性能门槛在 macOS 的
profile 构建与真机上判**；模拟器只看功能与相对值。

## 3. 用例

### A. coding agent

三个 agent（A1 Claude Code、A2 Codex CLI、A3 opencode）各跑下列场景。

| 场景 | 做什么 | 重点看 |
|---|---|---|
| S1 高速流式 | `stream`、`burst`；流式中在 agent 输入框打字 | 帧率、显示延迟、打字回显延迟、最终画面完整且一致 |
| S2 工具调用 | `tools` | diff 着色与对齐、长输出的折叠与滚动、每轮切换时的重绘 |
| S3 多 subagent 并发与切换 | `subagents`；进行中切到子视图、在子视图间切换、回到主视图（Claude Code ↓ 选中后台 agent、Enter 查看、Ctrl+O 详细记录；Codex Alt+← / →；opencode Ctrl+X ↓ / ← / → / ↑） | 并发刷新下的帧率与延迟、切换视图时的整屏重绘、无残影 |
| S4 长会话 | `long` | 内存、滚回上翻、选区复制 |
| S5 打断与退出 | 流式中 Esc / Ctrl-C 打断；正常退出；流式中直接杀掉 agent | 回到 shell 后画面与终端模式干净（同步输出、鼠标上报、键盘协议、光标） |
| S6 改尺寸 | 流式中旋转、iPad 分屏窗口变化、软键盘升降 | 重排正确、无错位残留 |
| S7 中文 | `cjk` | 折行后每行完整、宽字符后的着色段位置正确、表格对齐 |

三个 agent 默认都是全屏界面（备用屏 + 鼠标上报）；另用**行内模式**（Claude Code 的经典渲染、
Codex 的 `--no-alt-screen`、opencode 的 `--mini`）跑 S1 与 S7——这条路径走主屏与滚回，Codex 还会用
滚动区域把历史插到视口上方。

另测**多窗格**：一个窗格跑 `burst`，另一个窗格的 shell 里打字，回显延迟不受影响。

### B. 全屏 TUI

程序：btop（`update_ms=100`）、nvtop（100 ms 一刷）、nload、bmon、iftop；htop、vim 作回归。

| 编号 | 检查 |
|---|---|
| B1 进入 | 进入备用屏、铺满窗格（行列与窗格一致，最后一行不被键位条或 Home 指示条挡住）、边框对齐、颜色正确；滚回不增长 |
| B2 运行 | 高刷新下的帧率与显示延迟；触摸拖动 → 滚轮或方向键；鼠标点击（btop 点进程、切面板） |
| B3 改尺寸 | 旋转、iPad 分屏窗口变化后重排正确，无旧画面残留；小于程序最小尺寸时（iPhone 竖屏的 btop）显示它自己的提示，放大后恢复 |
| B4 退出 | 回到进入前的画面（之前打印的标记行还在，提示符紧随其后）、光标可见、备用屏与鼠标上报已关、滚回里没有 TUI 画面、触摸拖动重新滚动滚回 |
| B5 异常退出 | SIGTERM / SIGKILL 后远端不复位时的表现与其他终端一致，`reset` 能恢复；SSH 断开（冻结容器）后重连，终端模式由 App 复位，新的 shell 不在备用屏里、不收鼠标上报 |

### C. 设备矩阵

iOS 27 模拟器：iPhone 17 Pro Max、iPhone 17、iPad Pro 13 英寸（M5）、iPad Pro 11 英寸（M5）。
每台：构建、安装、启动、连上；竖屏与横屏布局（安全区、灵动岛、Home 指示条、键位条、标签条）；
A 的 S1 + S3（三个 agent）、B 全套；关键画面截图归档。

### D. iPad 键盘与鼠标

XCUITest 合成事件，远端 `m6-keyecho` 核对收到的字节：

| 编号 | 事件 | 期望 |
|---|---|---|
| D1 | 字母、数字、符号（含 Shift 大小写）、空格 | 原字符 |
| D2 | 回车、退格、Tab、Shift+Tab、Esc | CR、DEL、HT、`CSI Z`、ESC |
| D3 | 方向键、Home、End、PgUp、PgDn、Delete、F1–F12 | 标准序列；远端开了应用光标键时方向键为 `SS3` |
| D4 | Ctrl+字母、Ctrl+[ ] \ 等 | C0 控制字符 |
| D5 | Option+字母、Option+方向键 | ESC 前缀；`CSI 1;3x` |
| D6 | ⌘T / ⌘W / ⌘D / ⌘C / ⌘V / ⌘A / ⌘1…9 | App 自己处理，不发到远端 |
| D7 | 连续输入 50 个键再回车 | 顺序不乱、不丢不重 |
| D8 | 鼠标左键单击、双击、右键 | 远端开了鼠标上报时收到按下 / 松开；没开时走本地（选词、菜单） |
| D9 | 按住拖动 | 1002 下收到拖动；没开上报时是本地选区 |
| D10 | 悬停移动 | 1003 下收到移动，同一格不重复 |
| D11 | 滚轮 / 触控板双指滚动 | 全屏程序里是滚轮事件（或方向键），shell 里滚动滚回 |
| D12 | Shift / Option / Control + 点击或滚动 | 修饰键位正确；Shift+点击走本地选区 |

XCUITest 在 iPad 模拟器上合成的事件有几处到不了 App 或者走样：回车、退格、Esc、Home、End、翻页、
向前删除根本到不了 App（换成模拟器自带的按键注入，回车照常到达），F1–F12 到达时错一位（F2 成了 F1），
悬停不产生任何指针事件。所以分两层验：

- **XCUITest**（UIKit → Flutter 引擎的真实路径）：可打印字符与 Shift、Tab / Shift+Tab、方向键、
  Ctrl / Option 组合、⌘（不发到远端）、连续输入，以及点按、右键、双击、拖动、滚轮、修饰键 + 点击；
- **集成测试**（从 Flutter 的按键 / 指针事件注入，经 App 的按键处理、引擎编码到远端）：回车、退格、Esc、
  编辑键、F1–F12、带修饰键的方向键、Ctrl / Alt + 标点，应用光标键模式；悬停。

D1 与 D7 逐键合成（终端的文本输入不在无障碍树里，XCUITest 的 `typeText` 用不了）；D7 以 Ctrl+M（CR）
结尾，检验直接处理的键不越过前面的文本。验收模拟器只留英文（美国）键盘：系统语言跟主机时默认是拼音，
字母会被组成汉字。实体键盘的回车、退格、Esc、编辑键、F 键与触控板悬停在真机清单里（§7）。

另外在 agent 里验实际用法：opencode 里点击与滚动、Codex 与 opencode 里鼠标滚动 transcript、
多行输入（Shift+Enter 或各 agent 的换行键）、Esc 打断、Ctrl-C。

### E. 终端协议（agent 与 TUI 依赖的部分）

| 编号 | 项 | 谁在用 |
|---|---|---|
| E1 | 同步输出（DEC 2026）：BSU / ESU 成对时整帧出现；BSU 之后 150 ms 内没有 ESU 也要照常显示 | Codex、opencode、btop 每帧都用 |
| E2 | 查询应答：DA1 / DA2、XTVERSION、CPR、DECRQM、OSC 10 / 11、kitty 键盘查询，按提问顺序作答；未知的 OSC / DCS / APC 静默吞掉 | 三个 agent 启动时都成批查询（Claude Code 以 DA1 应答为界，之前没答的当作不支持） |
| E3 | kitty 键盘协议（影响 Shift+Enter 等组合键） | Codex 不查询直接推送；opencode 查询后才推送 |
| E4 | 焦点上报 1004、bracketed paste 2004、OSC 52 复制、OSC 8 链接、频繁改标题（Codex 每 100 ms）、BEL / OSC 9 通知、REP（`CSI b`）、DEC 线框字符 | 各 agent 与 TUI |

## 4. 度量与通过标准

**性能**（macOS profile 构建代表本机；真机你来验，§7）：

| 指标 | 门槛 |
|---|---|
| 显示延迟（S1、S3、B2 期间） | p95 ≤ 50 ms，max ≤ 250 ms |
| ACK 超时 | 0 |
| 出帧率 | 不低于远端的刷新节奏（上限按屏幕刷新率） |
| Flutter 卡顿帧 | ≤ 1% |
| 打字回显延迟 | shell 里 p95 ≤ 50 ms；agent 输入框 p95 ≤ 150 ms（含 agent 自己的节流） |
| 多窗格 | 另一窗格高速输出时，本窗格回显延迟 p95 仍 ≤ 50 ms |
| 内存（S4） | RSS 增长不超过滚回上界对应的量 + 50 MB，结束后不再上涨 |

**模拟器**（debug 构建）：不设绝对门槛，但显示延迟不得随时间持续增长（积压），不得出现超过 1 秒
的画面停滞。

**显示**：场景结束时画面一致性通过（引擎与 Dart 逐列相同）、画面含 `M6-DONE <场景>`、无 U+FFFD、
无泄漏到屏幕上的转义序列文本；截图人工复核无错位、残影，颜色与边框正确。

**TUI**：B1–B5 的断言全部通过。**键盘鼠标**：D1–D12 每个事件远端收到的字节与期望一致。

## 5. 立项时已确认的问题

立项调研时在模拟器与引擎上已经复现的问题，M6 当场修，修后由上面的用例守住：

| # | 问题 | 复现 | 影响 | 处理 |
|---|---|---|---|---|
| 1 | 帧编码按「格」而不是「列」记 run 的位置与长度，Dart 再按自己的简化宽度表重排 | 整行中文只画出前一半；`中文` 后接着色的 `X` 显示成 `中X` | 所有含中文 / emoji 的输出，agent 的中文回答尤甚 | 编码按列给出位置并带上每格宽度，Dart 不再自己算宽度（宽度的权威在引擎，铁律 4） |
| 2 | 同步输出没有超时：BSU 之后没有 ESU，后面的输出（包括 shell 提示符）都不再显示 | `printf '\e[?2026h'` 后画面卡死 | Codex、opencode、btop 每帧都用；它们在一帧中途被杀（Ctrl-C、崩溃、SSH 断开）就会卡死 | rsHell 补丁：引擎给出同步截止时刻，会话循环到点结束同步（与 alacritty 一致，150 ms） |
| 3 | 硬件键盘快速连续输入时，回车越过前面还在输入法通道里的字符 | 模拟器一次键入 `…cjk.sh⏎`，先执行了 `…cjk.s` | 快速打字、文本扩展、扫码枪 | 保序等待的超时按「最后一次进展」计，而不是按最后一次按键 |

另有几项需要结合 agent 实测再定（结论写进 §8，属于产品决定的开 followup）：

- XTVERSION（`CSI > q`）不应答：Claude Code 因此不查 2026、不用同步输出，一帧可能被拆成两次显示；
- OSC 10 / 11（前景 / 背景色查询）一律答黑色：Codex 与 opencode 用它选深浅配色与输入框底色；
- kitty 键盘协议默认关闭：Shift+Enter 与 Enter 分不开；
- OSC 52 被忽略：opencode 的选中即复制、Codex 的复制进不了系统剪贴板；
- 焦点上报（1004）从不发送：Codex 靠它决定通知与自动摘要。

## 6. 范围排除与限制

- 真机补充结果见 [真机 E2E 补充验收](acceptance-device-2026-09-27.md)，未覆盖部分保留在 §7；
- 模拟器只能跑 debug 构建，性能门槛在 macOS profile 与真机上判；
- agent 自身的 bug 不修，只记录终端这一侧的问题；
- nvtop 用假 NVML，网速监控只看容器内回环流量；
- 假上游不模拟真实模型的行为，只模拟输出的节奏与形态。

## 7. 真机清单（状态见补充验收）

1. **性能**（profile 构建，`flutter run --profile`）：iPhone 与 iPad 上各跑三个 agent 的 `burst` 与
   `subagents`、btop，看日志里的 `[m6-perf]`；ProMotion 设备看出帧能否到 120 Hz；
2. **iPad + 妙控键盘 / 触控板**：D1–D12 手动走一遍，重点是模拟器上验不到的部分——实体键盘的回车、
   退格、Esc（妙控键盘没有 Esc 键时用 Ctrl+[）、Home / End、翻页、F 键，触控板悬停（在 `m6-keyecho motion`
   里看移动上报）；另看触控板的惯性滚动、台前调度窗口缩放；中文输入法下组字上屏、组字时的回车与 Esc
   由输入法处理；
3. **iPhone 软键盘**：键位条的 Esc / Ctrl / 方向键在三个 agent 里的实际使用；
4. **agent 的手动部分**：流式中按 Esc 打断、流式中旋转与 iPad 分屏改窗口、长会话里上翻滚回与选区复制；
5. **进后台与锁屏**：agent 流式中切走再回来、锁屏再解锁，会话与画面状态。

## 8. 结果

执行于 2026-09-26：本机 Apple Silicon Mac（8 核）；macOS 为 profile 构建；模拟器为 iOS 27、debug 构建；
验收服务器与假上游见 §2。汇总由 `./scripts/m6.sh report` 生成（`build/m6/summary.md`）。

### 8.1 结论

| 问题 | 结论 |
|---|---|
| coding agent | Claude Code、Codex、opencode 的场景都跑通（iPad Pro 11 上 S1、S2、S3、S4、S7 与行内模式共 8 个场景 × 3，其余三台与 macOS 上 S1 + S3，macOS 另跑 S4；S5 为场景后退出与流式中被杀）：场景结束时画面与引擎逐列一致，没有乱码、没有泄漏的转义序列，退出（含流式中被杀）后终端模式干净。macOS profile 构建上显示延迟 p95 ≤ 11 ms、max ≤ 15 ms，无 ACK 超时，卡顿帧 ≤ 0.2%，输入框打字回显 17–50 ms，长会话内存不涨——全部达到 §4 的门槛 |
| 全屏 TUI | btop、nvtop、nload、bmon、iftop、htop 在四台模拟器与 macOS 上进入、运行、改尺寸、退出都正常：备用屏进出干净，鼠标上报与光标复位，TUI 画面不进滚回，改尺寸后画面与引擎一致；被 SIGKILL 后 `reset` 能恢复。macOS 上 btop 延迟 p95 11 ms、max 27 ms。窗格小于程序最小尺寸时显示程序自己的提示；iPhone 横屏也只有 19–21 行，放不下 btop（followup） |
| 设备 | iPhone 17 Pro Max、iPhone 17、iPad Pro 13、iPad Pro 11 模拟器上构建、安装、连接，三套集成测试与真实旋转都通过；debug 构建只看相对值，没有超过 1 秒的停滞 |
| iPad 键鼠 | 两台 iPad 上，系统合成的硬件键盘（可打印字符、Shift、Tab、方向键、Ctrl / Option / ⌘ 组合、连续输入）与指针事件（单击、右键、双击、拖动、滚轮、修饰键 + 点击）经 XCUITest 核对通过；回车、退格、Esc、编辑键、F 键、Ctrl / Alt + 标点、应用光标键模式与悬停从 Flutter 层注入核对通过（41 个键）。验收中修好了 Ctrl / Alt + 标点被丢掉。实体键盘与触控板在真机清单（§7） |

### 8.2 macOS（profile 构建，判性能门槛）

**全屏 TUI**（运行中 6 秒的统计；改尺寸为宽高对调）：

| 程序 | 出帧 fps | 显示延迟 p95 / max ms | ACK 超时 | 卡顿帧 | 改尺寸后一致 | 退出后模式 | 滚回增长 |
|---|---|---|---|---|---|---|---|
| btop | 18.1 | 11 / 27 | 0 | 2 / 359 | 是 | 干净 | 0 |
| nvtop | 16.9 | 11 / 12 | 0 | 0 / 362 | 是 | 干净 | 0 |
| nload | 8.6 | 11 / 11 | 0 | 0 / 356 | 是 | 干净 | 0 |
| bmon | 1.8 | 10 / 11 | 0 | 0 / 307 | 是 | 干净 | 0 |
| iftop | 1.1 | 10 / 11 | 0 | 0 / 244 | 是 | 干净 | 0 |
| htop | 3.0 | 10 / 10 | 0 | 0 / 340 | 是 | 干净 | 0 |

**coding agent**（从启动 agent 到画面静止的整段统计；打字回显为流式中在 agent 输入框里送出文字到它出现在
屏幕上）：

| agent × 场景 | 出帧 fps | 显示延迟 p95 / max ms | ACK 超时 | 卡顿帧 | 打字回显 ms |
|---|---|---|---|---|---|
| Claude Code × stream | 8.5 | 1 / 11 | 0 | 1 / 170 | 17 |
| Claude Code × burst | 8.2 | 7 / 11 | 0 | 0 / 95 | 17 |
| Claude Code × subagents | 12.3 | 4 / 10 | 0 | 1 / 513 | — |
| Claude Code × long | 17.1 | 2 / 10 | 0 | 0 / 674 | — |
| Codex × stream | 67.3 | 10 / 12 | 0 | 0 / 294 | 43 |
| Codex × burst | 51.8 | 10 / 11 | 0 | 0 / 129 | 50 |
| Codex × subagents | 36.2 | 10 / 12 | 0 | 0 / 518 | — |
| Codex × long | 66.2 | 10 / 12 | 0 | 0 / 1043 | — |
| opencode × stream | 38.0 | 11 / 12 | 0 | 0 / 355 | 33 |
| opencode × burst | 24.2 | 11 / 11 | 0 | 0 / 207 | 33 |
| opencode × subagents | 37.7 | 11 / 15 | 0 | 0 / 731 | — |
| opencode × long | 44.7 | 11 / 12 | 0 | 0 / 1032 | — |

**多窗格**：左窗格每秒 2000 行彩色输出、持续 20 秒，右窗格 shell 里逐字敲：回显 p95 17 ms（空闲时 19 ms，
即一帧）；输出窗格 110.4 fps，显示延迟 p95 12 ms、max 20 ms，无 ACK 超时，1079 帧没有卡顿。

单帧开销：Rust 渲染一帧最多 0.64 ms，Dart 应用一帧最多 0.54 ms；Flutter 帧的 build p90 ≤ 2.1 ms、
raster p90 ≤ 4.0 ms。**内存**：S4 长会话（30 轮）里 RSS 只涨 3–10 MB（Claude Code 202 → 212、Codex
218 → 223、opencode 207 → 210 MB）；整轮 12 个场景、六个 TUI 各开关一个会话，RSS 都在 188–227 MB 之间，
没有随会话累积。

Claude Code 的显示延迟低于另外两个：它的帧间隔至少 16 ms、每次成块写出（§9），成块的输出到达时通常
没有帧在途，立即出帧；Codex 与 opencode 持续输出，按 8 ms 节拍合帧。

### 8.3 模拟器（debug 构建，只看功能与相对值）

**终端协议与键鼠**：

| 设备 | 宽字符画面一致 | 同步输出卡住后照常显示 | 按键编码（集成测试） | 悬停（1003 / 1002） | XCUITest 键盘 / 指针 / 旋转 |
|---|---|---|---|---|---|
| iPad Pro 11 | 0 处不一致 | 167 ms | 41 / 41 | 逐格 7 次、无重复 / 0 次 | 通过 / 通过 / 通过 |
| iPad Pro 13 | 0 处不一致 | 134 ms | 41 / 41 | 逐格 7 次、无重复 / 0 次 | 通过 / 通过 / 通过 |
| iPhone 17 Pro Max | 0 处不一致 | 134 ms | 41 / 41 | 逐格 7 次、无重复 / 0 次 | — / — / 通过 |
| iPhone 17 | 0 处不一致 | 133 ms | 41 / 41 | 逐格 6 次、无重复 / 0 次 | — / — / 通过 |

真实旋转（XCUITest 截图复核）下 btop 的行列：iPad 两台横竖屏都铺满窗格、重排正确；iPhone 17 Pro Max
竖屏 52×50、横屏 98×21，iPhone 17 竖屏 47×44、横屏 88×19，btop 显示它自己的「Terminal size too small」。

**全屏 TUI**（六个程序，运行中 6 秒）：

| 设备 | 显示延迟 p95 | 显示延迟 max | ACK 超时 | 改尺寸后一致 | 退出后模式 | 滚回增长 |
|---|---|---|---|---|---|---|
| iPad Pro 11 | 10–14 ms | 45 ms | 0 | 是 | 干净 | 0 |
| iPad Pro 13 | 10–13 ms | 19 ms | 0 | 是 | 干净 | 0 |
| iPhone 17 Pro Max | ≤ 16 ms | 17 ms | 0 | 是 | 干净 | 0 |
| iPhone 17 | ≤ 14 ms | 17 ms | 0 | 是 | 干净 | 0 |

**coding agent**：

| 设备 | 场景 | 显示延迟 p95（各场景最大） | 显示延迟 max（各场景最大） | ACK 超时 | 打字回显 | 画面一致 / 乱码 / 退出后模式 |
|---|---|---|---|---|---|---|
| iPad Pro 11 | 8 个 × 3 | 16 ms | 188 ms | 0 | 18–57 ms | 全部一致 / 无 / 干净 |
| iPad Pro 13 | S1 + S3 × 3 | 130 ms | 367 ms | 5 | 32–131 ms | 全部一致 / 无 / 干净 |
| iPhone 17 Pro Max | S1 + S3 × 3 | 14 ms | 176 ms | 0 | 19–66 ms | 全部一致 / 无 / 干净 |
| iPhone 17 | S1 + S3 × 3 | 15 ms | 318 ms | 1 | 33–103 ms | 全部一致 / 无 / 干净 |

显示延迟的最大值都出在 6 个 subagent 并发收尾时（100–370 ms），少数几次 ACK 超时也在这里（debug 构建的
Dart 比 profile 慢得多）；macOS profile 构建上同样的场景 max 12–15 ms、没有 ACK 超时。iPad Pro 13 上
Claude Code × stream 的 p95 130 ms 出在整轮的第一个场景，同一场景在其余三台上 ≤ 15 ms、macOS 上 1 ms。

**多窗格**（左窗格每秒 2000 行，右窗格 shell 里逐字敲）：

| 设备 | shell 回显 p95：空闲 / 另一窗格高速输出时 | 输出窗格 fps | 输出窗格延迟 p95 / max | ACK 超时 |
|---|---|---|---|---|
| iPad Pro 11 | 41 / 17 ms | 110.2 | 14 / 26 ms | 0 |
| iPad Pro 13 | 23 / 40 ms | 105.9 | 21 / 35 ms | 0 |
| iPhone 17 Pro Max | 23 / 18 ms | 110.2 | 13 / 22 ms | 0 |
| iPhone 17 | 37 / 18 ms | 110.6 | 12 / 22 ms | 0 |

回显的下限约 16 ms，即一帧；另一窗格的高速输出不拖慢本窗格。

### 8.4 验收中发现并已修复的问题

| 问题 | 怎么发现的 | 修复 |
|---|---|---|
| 帧编码按「格」记位置：整行中文只画出一半、宽字符后的着色段错位 | 立项调研（§5 #1） | 按列定位，带每格的码点数与宽度 |
| 同步输出没有超时：BSU 之后没有 ESU，画面就卡住 | 立项调研（§5 #2） | rsHell 补丁 P8，150 ms 照常显示 |
| 硬件键盘快速连续输入时回车越过前面的字符 | 立项调研（§5 #3） | 保序等待的超时按最后一次进展计 |
| 清屏后输出很快时视图停在旧位置、不再跟随底部 | E1 `m6-sync pair` 的画面是空的 | 只有用户发起的滚动才算离开底部（iOS 回弹不算） |
| 组合字符（é、ñ 等）没画出来 | `m6-cjk` 截图 | 带组合字符的格不走 ASCII 批量绘制 |
| Ctrl / Alt + 标点被丢掉：Ctrl+[（妙控键盘没有 Esc 键时的 Esc）、Ctrl+\、Ctrl+]、Ctrl+_、Alt+.、Alt+> 等 | D4 | 标点键按 US 布局的基准字符发出，带修饰键的 Shift 组合换成上档字符 |
| debug 构建的自动连接在 iOS 上不生效 | D 类（XCUITest 连不上） | Rust 读进程环境变量，随 AppReady 交给 Dart |

### 8.5 记下待处理的问题（followup）

| 条目（`docs/followups/`） | 影响 |
|---|---|
| SSH 会话不发送 LANG | 没配系统 locale 的服务器上 btop 拒绝启动，中文可能乱码 |
| XTVERSION 不应答 | Claude Code 不用同步输出，一帧可能分两次显示 |
| 终端颜色查询一律答黑色 | Codex、opencode 按错误的前景 / 背景色推断配色 |
| kitty 键盘协议默认关闭 | 三个 agent 里 Shift+Enter 都不能换行 |
| OSC 52 复制进不了系统剪贴板 | opencode 选中即复制、agent 的复制命令都无效 |
| 焦点上报（1004）不发送 | Codex 的失焦通知，vim / tmux 的焦点事件 |
| 带 VS16 的 emoji 宽度与远端程序不一致 | opencode 里 ⚠️ 等之后多一格空白或残留旧字符 |
| iPhone 横屏的终端行数不够全屏 TUI | iPhone 横屏只有 19–21 行，放不下 80×24 的 btop |

### 8.6 测试手段与限制

- **XCUITest 的合成事件有缺口**（§3 D）：回车、退格、Esc、编辑键到不了 App，F 键错一位，悬停没有事件；
  这些改从 Flutter 层注入核对，实体键盘与触控板在真机清单里。
- **假旋转**（集成测试改视图尺寸）转不了系统键盘与安全区：竖屏的软键盘还占着时宽高对调，工作区只剩
  几十像素、布局溢出——测试里先收起软键盘；真实旋转由 XCUITest 在四台模拟器上截图复核。
- **模拟器上的 RSS 含测试截图**：iOS 上插件把每张截图留在 App 进程里直到测试结束（iPad 一张约 30 MB），
  所以 RSS 随截图张数上涨；内存以不截图的 macOS 为准。
- **没有自动化的部分**：S5 的流式中 Esc 打断、S6 的 agent 流式中改尺寸（改尺寸的重排由 B3 与真实旋转
  覆盖）、S4 长会话里的滚回上翻与选区复制，列进真机清单（§7）。
- **主机负载**：新模拟器首次启动、iPadOS 开机时的壁纸扩展、Spotlight 给构建产物建索引，都会把负载推到
  核数的几十倍；`scripts/m6.sh` 在构建之后等负载降下来再测。有一轮 iPad 11 在高负载下跑（Rust 渲染一帧
  慢了十几倍），它的多窗格数据作废、低负载下重测。

## 9. 调研依据

调研于 2026-09-26，均来自源码（对应 tag）与官方文档：

- **假上游**：比较了 aimock、llm-mock-server、MockServer、llm-d-inference-sim、VidaiMock、mockllm 等。
  只有 aimock 同时具备三种协议的流式工具调用、按请求内容匹配、可编程回复、速率控制与多架构运行，
  MIT。它的缺口都能绕开：Responses 的工具命名空间与 `agent_message` → 前面一层转发补上（§2.2）；
  不支持 Codex 的 WebSocket 续接 → 自定义 provider 本来就不用 WebSocket；emoji 分块 → 见 §2.2；
  请求日志截断 64 KB 以上的请求体 → 剧本另记每个请求的决定。
- **Claude Code 2.1.274**：npm 包只是启动器，实际是按平台分发的 Bun 编译二进制（linux-arm64 为 glibc 版）。
  用 `ANTHROPIC_AUTH_TOKEN` + `ANTHROPIC_BASE_URL` 时没有连通性检查与 API key 确认；预置
  `~/.claude.json`（已完成引导、工作目录已信任）与 `~/.claude/settings.json`（权限模式）即直接进入输入框；
  关闭非必要流量后完全离线可用。每轮 `POST /v1/messages?beta=true` 流式；首条消息另发一条不带工具的
  标题请求。subagent 工具名 `Agent`，交互模式下在后台并发运行；界面上 ↓ 进入后台 agent 列表、Enter
  查看某个 agent。默认全屏（备用屏 + 1000/1002/1003/1006 鼠标上报），另有经典的行内渲染；启动时按
  XTVERSION、`CSI ?u`、DA1 的顺序查询，XTVERSION 有应答才用 DECRQM 查 2026、支持即每帧同步输出；
  帧间隔至少 16 ms；标题每 960 ms 转圈；复制走 OSC 52。
- **Codex CLI 0.157.1**：只支持 Responses；默认备用屏 + 全部鼠标上报；每帧同步输出；启动时成批查询
  CPR、OSC 10 / 11、`CSI ?u`、DA1 / DA2（共 250 ms）；不查询直接推 kitty 键盘标志；标题每 100 ms 转圈；
  subagent 用 `/subagents` 或 Alt+← / →（输入框为空时）切换；会发不带工具的标题请求；
  需要模型目录文件避免每轮警告；自定义 provider 不用 WebSocket。
- **opencode 1.18.32**：Chat Completions 经 `@ai-sdk/openai-compatible`（编进二进制，不在运行时下载）；
  备用屏 + 1003 全程鼠标上报；每帧同步输出；启动时查询 OSC 10 / 11、XTVERSION、CPR、DECRQM、`CSI ?u` 等；
  收到 `CSI ?u` 应答才推 kitty 键盘标志；选中即经 OSC 52 复制；subagent 用 Ctrl+X 引导键切换；
  标题生成用 small model。
- **TUI**：trixie 的 btop 1.3.2 总是真彩色、每帧同步输出、鼠标常开、最小 80×24；nload / bmon 无需权限，
  iftop 需要 `cap_net_raw`；多数 TUI 在 SIGHUP / SIGKILL 下不复位终端（SSH 断开即 SIGHUP）；
  nvtop 用 DEC 线框字符，nload 用 REP。
