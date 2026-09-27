// M6 剧本：请求 → 这一轮回什么。回复只由请求内容决定（并发、重试都安全，没有会话状态）：
// * 场景：用户消息里的标记 `[m6:<场景>]`；subagent 由派发时写进它 prompt 的
//   `[m6-sub:<场景>:<序号>]` 识别（以第一条带标记的用户消息为准）。
// * 轮次：剧本发出的工具调用 id 形如 `m6_<场景>_<谁>_<轮>_<序>`，请求里已有的最大轮次 + 1
//   就是这一轮。
// * 工具意图（执行命令、读、写、改、派 subagent、等 subagent）按 agent 的工具列表翻译，
//   参数名取自请求里该工具的 JSON schema。
import { demoFile, estimateTokens, makeText } from "./text.mjs";

/**
 * 速率档：aimock 的 tps 是「每秒块数」，每块 chunkSize 个字符；1 token 约 4 个字符。
 * fast ≈ 2000 token/s，burst ≈ 8000 token/s，sub ≈ 250 token/s。
 */
export const RATES = {
  aux: { chunkSize: 4096, tps: 1000, ttft: 20, jitter: 0 },
  normal: { chunkSize: 8, tps: 60, ttft: 300, jitter: 0.2 },
  fast: { chunkSize: 24, tps: 330, ttft: 250, jitter: 0.1 },
  burst: { chunkSize: 48, tps: 660, ttft: 200, jitter: 0.1 },
  // subagent 的收尾：6 个并发各流约 8 秒，界面切换发生在它们还在刷新的时候。
  sub: { chunkSize: 8, tps: 125, ttft: 200, jitter: 0.2 },
};

const SUBAGENTS = 6;
const LONG_TURNS = 30;

const MAIN = /\[m6:([a-z0-9]+)\]/;
const SUB = /\[m6-sub:([a-z0-9]+):(\d+)\]/;
const WORKDIR = /工作目录 (\S+)/;
const CALL_ID = /^m6_([a-z0-9]+)_(main|s\d+)_(\d+)_(\d+)$/;
const SUB_DONE = /M6-SUB-DONE ([a-z0-9]+):(\d+)/g;

function textOf(content) {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content.map((part) => (typeof part === "string" ? part : (part?.text ?? ""))).join("");
  }
  return "";
}

/** 按工具名找到请求里的工具定义（名字不区分大小写）。 */
function toolIndex(req) {
  const index = new Map();
  for (const tool of req.tools ?? []) {
    const name = tool?.function?.name;
    if (name) index.set(name.toLowerCase(), tool.function);
  }
  return index;
}

/** agent 方言：看工具列表。 */
function dialectOf(tools) {
  if (tools.has("exec_command")) return "codex";
  if (tools.has("bash") && tools.has("read") && tools.get("bash").name === "Bash") return "claude";
  if (tools.has("bash")) return "opencode";
  return null;
}

/** 在工具的参数 schema 里挑第一个存在的参数名。 */
function argName(tool, ...candidates) {
  const props = tool?.parameters?.properties ?? {};
  return candidates.find((name) => name in props) ?? candidates[0];
}

/** 找到对话的身份：第一条带标记的用户消息。 */
function identify(messages) {
  for (const message of messages) {
    if (message.role !== "user") continue;
    const text = textOf(message.content);
    const sub = SUB.exec(text);
    const main = MAIN.exec(text);
    if (!sub && !main) continue;
    const workdir = WORKDIR.exec(text)?.[1] ?? "/tmp";
    if (sub && (!main || sub.index < main.index)) {
      return { role: "sub", scenario: sub[1], index: Number(sub[2]), who: `s${sub[2]}`, workdir };
    }
    return { role: "main", scenario: main[1], who: "main", workdir };
  }
  return null;
}

/** 这个身份在对话里已经发过的最大轮次（没有为 -1），以及它发过的调用 id。 */
function history(messages, who) {
  let step = -1;
  for (const message of messages) {
    for (const call of message.tool_calls ?? []) {
      const m = CALL_ID.exec(call.id ?? "");
      if (m && m[2] === who) step = Math.max(step, Number(m[3]));
    }
  }
  return step;
}

/** 对话里出现过的 subagent 完成标记（工具结果或通知消息里）。 */
function finishedSubagents(messages, scenario) {
  const done = new Set();
  for (const message of messages) {
    if (message.role === "assistant") continue;
    for (const m of textOf(message.content).matchAll(SUB_DONE)) {
      if (m[1] === scenario) done.add(Number(m[2]));
    }
  }
  return done.size;
}

/** 工具意图 → 该 agent 的一次工具调用。 */
function toolCall(dialect, tools, intent, id, ctx) {
  const call = (name, args) => ({ id, name: tools.get(name.toLowerCase())?.name ?? name, arguments: JSON.stringify(args) });
  const path = (file) => `${ctx.workdir}/${file}`;
  switch (intent.kind) {
    case "shell": {
      if (dialect === "codex") return call("exec_command", { cmd: intent.cmd });
      const tool = tools.get("bash");
      return call("bash", {
        [argName(tool, "command")]: intent.cmd,
        [argName(tool, "description")]: intent.note ?? "M6 命令",
      });
    }
    case "read": {
      if (dialect === "codex") return call("exec_command", { cmd: `sed -n 1,80p ${path(intent.file)}` });
      const tool = tools.get("read");
      return call("read", { [argName(tool, "file_path", "filePath")]: path(intent.file) });
    }
    case "write": {
      const content = demoFile(ctx.seed);
      if (dialect === "codex") {
        return call("exec_command", { cmd: `cat > ${path(intent.file)} <<'M6EOF'\n${content}M6EOF` });
      }
      const tool = tools.get("write");
      return call("write", {
        [argName(tool, "file_path", "filePath")]: path(intent.file),
        [argName(tool, "content")]: content,
      });
    }
    case "edit": {
      const before = "value = 1\n";
      const after = "value = 7  # M6：改过的初值\n";
      if (dialect === "codex") {
        // Codex 认得 exec_command 里的 apply_patch 调用，按补丁显示 diff 并自己应用。
        const patch = [
          "*** Begin Patch",
          `*** Update File: ${path(intent.file)}`,
          "@@",
          `-    ${before.trimEnd()}`,
          `+    ${after.trimEnd()}`,
          "*** End Patch",
        ].join("\n");
        return call("exec_command", { cmd: `apply_patch <<'M6EOF'\n${patch}\nM6EOF` });
      }
      const tool = tools.get("edit");
      return call("edit", {
        [argName(tool, "file_path", "filePath")]: path(intent.file),
        [argName(tool, "old_string", "oldString")]: `    ${before}`,
        [argName(tool, "new_string", "newString")]: `    ${after}`,
      });
    }
    case "spawn": {
      const prompt =
        `[m6-sub:${ctx.scenario}:${intent.index}] 工作目录 ${ctx.workdir} ` +
        `M6 子任务 ${intent.index}：执行两条命令，读一个文件，然后汇报。`;
      if (dialect === "claude") {
        return call("agent", { description: `M6 子任务 ${intent.index}`, prompt, subagent_type: "general-purpose" });
      }
      if (dialect === "codex") {
        return call("spawn_agent", { task_name: `m6_sub_${intent.index}`, message: prompt, fork_turns: "none" });
      }
      return call("task", { description: `M6 子任务 ${intent.index}`, prompt, subagent_type: "general" });
    }
    case "wait":
      return call("wait_agent", { timeout_ms: 30000 });
    default:
      throw new Error(`unknown intent ${intent.kind}`);
  }
}

/** 主 agent 的剧本：返回 { rate, say, think, calls, done }。 */
function mainStep(scenario, step, ctx) {
  switch (scenario) {
    case "stream":
    case "burst":
    case "cjk":
      return {
        rate: scenario === "burst" ? "burst" : "fast",
        think: 300,
        say: { tokens: 6000, style: scenario === "cjk" ? "cjk" : "markdown" },
        done: true,
      };
    case "tools": {
      const plan = [
        { kind: "shell", cmd: "ls -la /usr/bin | head -60", note: "列出 /usr/bin" },
        { kind: "read", file: "../README.m6" },
        { kind: "write", file: "demo.py" },
        { kind: "edit", file: "demo.py" },
        { kind: "shell", cmd: "seq -f '第 %g 行 · 长输出' 1 1500", note: "长输出" },
        { kind: "shell", cmd: "python3 demo.py", note: "运行 demo" },
      ];
      if (step < plan.length) {
        return { rate: "fast", think: 60, say: { tokens: 250, style: "plain" }, calls: [plan[step]] };
      }
      return { rate: "fast", say: { tokens: 1200, style: "markdown" }, done: true };
    }
    case "long":
      if (step < LONG_TURNS) {
        return {
          rate: "fast",
          say: { tokens: 400, style: step % 2 ? "cjk" : "markdown" },
          calls: [{ kind: "shell", cmd: `seq -f '第 ${step + 1} 轮 · 第 %g 行' 1 100`, note: `第 ${step + 1} 轮` }],
        };
      }
      return { rate: "fast", say: { tokens: 800, style: "markdown" }, done: true };
    case "subagents": {
      if (step === 0) {
        return {
          rate: "fast",
          think: 80,
          say: { tokens: 150, style: "plain" },
          calls: Array.from({ length: SUBAGENTS }, (_, index) => ({ kind: "spawn", index })),
        };
      }
      const finished = finishedSubagents(ctx.messages, "subagents");
      if (finished >= SUBAGENTS || step > 3 * SUBAGENTS) {
        return { rate: "burst", say: { tokens: 1500, style: "markdown" }, done: true };
      }
      if (ctx.dialect === "codex") {
        return { rate: "fast", say: { tokens: 40, style: "plain" }, calls: [{ kind: "wait" }] };
      }
      // Claude Code：subagent 在后台跑，每完成一个就来一条通知；先简短应答，等齐再总结。
      return { rate: "fast", say: { tokens: 40, style: "plain" }, note: `已完成 ${finished}/${SUBAGENTS}` };
    }
    default:
      return { rate: "normal", say: { tokens: 60, style: "plain" }, done: true, unknown: true };
  }
}

/** subagent 的剧本：两轮工具调用 + 总结。 */
function subStep(step) {
  const plan = [
    { kind: "shell", cmd: "ls -la /etc | head -40", note: "看看 /etc" },
    { kind: "read", file: "../README.m6" },
  ];
  if (step < plan.length) {
    return { rate: "fast", say: { tokens: 300, style: "plain" }, calls: [plan[step]] };
  }
  return { rate: "sub", say: { tokens: 2000, style: "markdown" }, done: true };
}

function usage(text) {
  const output = estimateTokens(text);
  // 报小一点的输入 token：长会话里 agent 不会因为「上下文快满了」去做压缩。
  return { input_tokens: 1000, output_tokens: output, prompt_tokens: 1000, completion_tokens: output, total_tokens: 1000 + output };
}

/** 不带工具的旁路请求：会话标题等。 */
function auxiliary(req, messages) {
  const system = messages.filter((m) => m.role === "system").map((m) => textOf(m.content)).join("\n");
  const scenario = identify(messages)?.scenario ?? "会话";
  const content = /title generator/i.test(system) ? `M6 ${scenario}` : JSON.stringify({ title: `M6 ${scenario}` });
  return { rate: "aux", response: { content, usage: usage(content) }, trace: { kind: "aux", scenario } };
}

/** 最近几条工具结果的摘要（排查剧本与工具参数用）。 */
function recentResults(messages) {
  return messages
    .filter((m) => m.role === "tool")
    .slice(-3)
    .map((m) => `${m.tool_call_id ?? "?"}: ${textOf(m.content).replace(/\s+/g, " ").slice(0, 160)}`);
}

/** 请求 → { rate, response }（aimock 的 FixtureResponse）。 */
export function plan(req) {
  const messages = Array.isArray(req.messages) ? req.messages : [];
  const tools = toolIndex(req);
  if (tools.size === 0) return auxiliary(req, messages);

  const dialect = dialectOf(tools);
  const who = identify(messages);
  if (!dialect || !who) {
    const content =
      "（M6 假上游）没有找到场景标记。在消息里写 `[m6:stream]`、`[m6:burst]`、`[m6:tools]`、" +
      "`[m6:subagents]`、`[m6:long]` 或 `[m6:cjk]` 开始一个场景。";
    return { rate: "normal", response: { content, usage: usage(content) }, trace: { kind: "unmarked", dialect } };
  }

  const step = history(messages, who.who) + 1;
  const ctx = { ...who, dialect, messages, seed: hash(`${who.scenario}/${who.who}/${step}`) };
  const effective = who.role === "sub" ? subStep(step) : mainStep(who.scenario, step, ctx);

  const emoji = dialect !== "codex";
  let content = effective.say ? makeText({ seed: ctx.seed, tokens: effective.say.tokens, style: effective.say.style, emoji }) : "";
  if (effective.note) content = `${effective.note}。${content}`;
  if (effective.done) {
    content += who.role === "sub" ? `\n\nM6-SUB-DONE ${who.scenario}:${who.index}` : `\n\nM6-DONE ${who.scenario}`;
  }
  const reasoning = effective.think ? makeText({ seed: ctx.seed + 1, tokens: effective.think, style: "plain", emoji: false }) : undefined;

  const calls = (effective.calls ?? []).map((intent, k) =>
    toolCall(dialect, tools, intent, `m6_${who.scenario}_${who.who}_${step}_${k}`, ctx),
  );
  // Anthropic 开着 extended thinking 时，带工具调用的一轮必须以签名的思考块开头。
  const thinking = reasoning ?? (calls.length && dialect === "claude" ? "按剧本调用工具。" : undefined);
  const response = calls.length
    ? { content, toolCalls: calls, reasoning: thinking, usage: usage(content) }
    : { content, reasoning: thinking, usage: usage(content) };
  const trace = {
    kind: who.role,
    dialect,
    scenario: who.scenario,
    who: who.who,
    step,
    calls: calls.map((call) => call.name),
    done: Boolean(effective.done),
    results: recentResults(messages),
  };
  return { rate: effective.rate, response, trace };
}

function hash(text) {
  let h = 2166136261;
  for (let i = 0; i < text.length; i += 1) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}
