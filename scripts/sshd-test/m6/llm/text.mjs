// 假模型的正文：确定性的 markdown 混排（固定种子，同一场景每次相同）。
// 覆盖 agent 界面最常渲染的形态：标题、段落（中英混排、自动折行的长段）、列表、
// 行内代码、代码块、表格（含中文单元格）、引用；emoji 可关（Codex 的剧本不放）。

/** mulberry32：小而确定的伪随机数。 */
function rng(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const EN = (
  "the terminal renders each frame from the engine while the remote program keeps " +
  "streaming output and the pipeline must stay responsive under load so that typing " +
  "echo arrives quickly scrollback stays intact and wide characters keep their columns " +
  "during rapid redraws of markdown tables code blocks and nested lists produced by the model"
).split(" ");

const ZH = [
  "终端的每一帧都来自引擎，远端程序持续输出时，渲染管线仍要保持跟手。",
  "宽字符必须占两列，着色段落在它后面也不能错位。",
  "高速流式输出下，显示延迟不能随时间积压。",
  "子任务并发运行，界面需要在多个视图之间来回切换。",
  "滚回里保留完整的历史，上翻时内容不漂移。",
  "表格里的中文单元格与英文单元格要对齐。",
  "同步输出把一帧包起来，终端要么整帧显示，要么不显示。",
  "退出全屏之后，之前的提示符与输出应当原样回来。",
];

const EMOJI = ["✅", "🚀", "⚠️", "🔥", "📦", "🧪", "✨", "🔧"];

const CODE = {
  python: [
    "def render(frame, viewport):",
    "    rows = []",
    "    for row in frame.rows[viewport.top:viewport.top + viewport.height]:",
    "        rows.append(''.join(cell.text for cell in row.cells))",
    "    return '\\n'.join(rows)  # 宽字符占两列",
    "",
    "class Pacer:",
    "    def __init__(self, interval_ms=8):",
    "        self.interval_ms = interval_ms",
    "        self.in_flight = None",
  ],
  rust: [
    "fn pack_runs(frame: &RenderFrame) -> Vec<u8> {",
    "    let mut out = Vec::with_capacity(64 * 1024);",
    "    for row in frame.rows.iter() {",
    "        let columns: u16 = row.cells.iter().map(|c| u16::from(c.width)).sum();",
    "        out.extend_from_slice(&columns.to_le_bytes());",
    "    }",
    "    out",
    "}",
  ],
  typescript: [
    "export async function stream(req: Request): Promise<void> {",
    "  const reader = req.body!.getReader();",
    "  for (;;) {",
    "    const { value, done } = await reader.read();",
    "    if (done) break;",
    "    process.stdout.write(value); // 中文注释：逐块写出",
    "  }",
    "}",
  ],
  bash: [
    "#!/usr/bin/env bash",
    "set -euo pipefail",
    "for i in $(seq 1 20); do",
    "  printf '\\e[3%dm第 %02d 行\\e[0m\\n' $((i % 7 + 1)) \"$i\"",
    "done",
  ],
};

/** 粗估 token：ASCII 约 4 字符一个，CJK 约 1.3 字符一个。 */
export function estimateTokens(text) {
  let ascii = 0;
  let wide = 0;
  for (const ch of text) {
    if (ch.codePointAt(0) < 0x80) ascii += 1;
    else wide += 1;
  }
  return Math.ceil(ascii / 4 + wide / 1.3);
}

/**
 * 生成约 `tokens` 个 token 的 markdown。
 * style: "markdown"（中英混排）| "cjk"（中文为主）| "plain"（短句，不带结构）。
 */
export function makeText({ seed, tokens, style = "markdown", emoji = true }) {
  const rand = rng(seed);
  const pick = (list) => list[Math.floor(rand() * list.length)];
  const words = (n) => Array.from({ length: n }, () => pick(EN)).join(" ");
  const zh = (n) => Array.from({ length: n }, () => pick(ZH)).join("");
  const mark = () => (emoji ? `${pick(EMOJI)} ` : "");

  if (style === "plain") {
    let out = "";
    while (estimateTokens(out) < tokens) out += (out ? " " : "") + words(12) + ".";
    return out;
  }

  const cjk = style === "cjk";
  const blocks = [];
  let used = 0;
  let section = 1;
  const push = (block) => {
    blocks.push(block);
    used += estimateTokens(block);
  };

  while (used < tokens) {
    const kind = section % 6;
    push(`## ${section}. ${cjk ? zh(1).slice(0, 12) : words(4)}`);
    // 长段：足够折好几行，考验折行后每行完整、宽字符不丢。
    push(cjk ? zh(4 + Math.floor(rand() * 4)) : `${words(40)}. ${zh(2)} ${words(20)}.`);
    if (kind === 1 || kind === 4) {
      const items = 3 + Math.floor(rand() * 3);
      const lines = [];
      for (let i = 0; i < items; i += 1) {
        lines.push(`- ${mark()}**${words(2)}**：${cjk ? zh(1) : words(10)} \`m6_${section}_${i}\``);
      }
      push(lines.join("\n"));
    }
    if (kind === 2 || kind === 5) {
      const lang = pick(Object.keys(CODE));
      push(["```" + lang, ...CODE[lang], "```"].join("\n"));
    }
    if (kind === 3) {
      push(
        [
          "| 项目 | 状态 | 说明 |",
          "|---|---|---|",
          `| 帧率 | ${mark()}正常 | ${words(6)} |`,
          `| 显示延迟 | p95 ${10 + Math.floor(rand() * 30)} ms | 宽字符后的着色段 |`,
          `| scrollback | 20000 行 | ${zh(1).slice(0, 16)} |`,
        ].join("\n"),
      );
    }
    if (kind === 0) {
      push(`> ${mark()}${cjk ? zh(2) : words(24)}`);
    }
    section += 1;
  }
  return blocks.join("\n\n");
}

/** 一段 60 行左右的 Python 文件内容（写文件 / 改文件用）。 */
export function demoFile(seed) {
  const rand = rng(seed);
  const lines = ["# M6 demo：由假上游生成", "import sys", ""];
  for (let i = 0; i < 12; i += 1) {
    const n = 1 + Math.floor(rand() * 90);
    lines.push(`def step_${i}(x):`);
    lines.push(`    \"\"\"第 ${i} 步：乘 ${n}\"\"\"`);
    lines.push(`    return x * ${n}`);
    lines.push("");
  }
  lines.push("if __name__ == '__main__':");
  lines.push("    value = 1");
  lines.push("    for i in range(12):");
  lines.push("        value = globals()[f'step_{i}'](value) % 1000003");
  lines.push("    print('M6 demo result', value)");
  return lines.join("\n") + "\n";
}
