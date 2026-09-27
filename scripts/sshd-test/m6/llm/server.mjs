// M6 假 AI 上游：aimock（三种协议的流式应答与工具调用）+ 我们的剧本（scenarios.mjs）。
// aimock 跑在内部端口上，前面一层转发只补 Codex（/v1/responses）的两处协议扩展：
// * Codex 把 subagent 工具声明在命名空间里（`{type: "namespace", name, tools}`），调用必须带上
//   同一个 namespace，而 aimock 生成的 function_call 不带——按请求里的声明给流补上；
// * agent 之间的消息是 `agent_message` 输入项（正文在 encrypted_content 部分，自定义 provider
//   下是明文），aimock 不认识会丢掉——转发前改写成普通的用户消息，剧本才看得到子任务与汇报。
// 对外一个端口同时提供：
//   /v1/messages（Claude Code）、/v1/responses（Codex）、/v1/chat/completions（opencode）
//   /__aimock/journal   aimock 的请求日志
//   /m6/input           m6-keyecho 记下的输入（XCUITest 断言用；?since=<字节偏移>）
//   /m6/input/reset     清空输入记录（POST）
//   /m6/plans           最近的剧本决定（每个请求：方言、场景、轮次、发出的工具调用、最近的工具结果）
import fs from "node:fs";
import http from "node:http";
import { LLMock } from "@copilotkit/aimock";
import { RATES, plan } from "./scenarios.mjs";

const port = Number(process.env.M6_LLM_PORT ?? 4010);
const host = process.env.M6_LLM_HOST ?? "0.0.0.0";
const inputLog = process.env.M6_INPUT_LOG ?? "/tmp/m6/input.log";

const internalPort = port + 1;
const mock = new LLMock({ port: internalPort, host: "127.0.0.1", logLevel: "info", journalMaxEntries: 4000 });

// 同一个请求对象，匹配与生成回复只算一次剧本。
const plans = new WeakMap();
// 最近的剧本决定（/m6/plans）。
const traces = [];
function planOf(req) {
  let result = plans.get(req);
  if (!result) {
    try {
      result = plan(req);
    } catch (error) {
      const content = `（M6 假上游）剧本出错：${error?.message ?? error}`;
      result = { rate: "aux", response: { content }, trace: { kind: "error", error: String(error?.stack ?? error) } };
    }
    plans.set(req, result);
    const entry = { t: Date.now(), model: req.model, rate: result.rate, ...result.trace };
    traces.push(entry);
    if (traces.length > 2000) traces.splice(0, traces.length - 2000);
    console.log(JSON.stringify(entry));
  }
  return result;
}

for (const [name, rate] of Object.entries(RATES)) {
  mock.addFixture({
    match: { predicate: (req) => planOf(req).rate === name },
    response: (req) => planOf(req).response,
    chunkSize: rate.chunkSize,
    streamingProfile: { ttft: rate.ttft, tps: rate.tps, jitter: rate.jitter },
  });
}

mock.mount("/m6", {
  async handleRequest(req, res, pathname) {
    const url = new URL(req.url ?? "/", "http://m6");
    if (pathname === "/input" && req.method === "GET") {
      const since = Math.max(0, Number(url.searchParams.get("since") ?? 0) || 0);
      let body = Buffer.alloc(0);
      try {
        const data = fs.readFileSync(inputLog);
        body = data.subarray(Math.min(since, data.length));
        res.setHeader("X-M6-Size", String(data.length));
      } catch {
        res.setHeader("X-M6-Size", "0");
      }
      res.writeHead(200, { "Content-Type": "text/plain; charset=utf-8" });
      res.end(body);
      return true;
    }
    if (pathname === "/input/reset" && req.method === "POST") {
      try {
        fs.writeFileSync(inputLog, "");
      } catch {
        // 没有记录文件就当已经清空。
      }
      res.writeHead(204);
      res.end();
      return true;
    }
    if (pathname === "/plans" && req.method === "GET") {
      const limit = Math.max(1, Number(url.searchParams.get("limit") ?? 200) || 200);
      res.writeHead(200, { "Content-Type": "application/json; charset=utf-8" });
      res.end(JSON.stringify(traces.slice(-limit)));
      return true;
    }
    if (pathname === "/health") {
      res.writeHead(200, { "Content-Type": "text/plain" });
      res.end("ok\n");
      return true;
    }
    return false;
  },
});

await mock.start();

/** 请求里声明在命名空间下的工具：工具名 → 命名空间。 */
function namespacedTools(body) {
  const map = new Map();
  for (const tool of body?.tools ?? []) {
    if (tool?.type !== "namespace" || typeof tool.name !== "string") continue;
    for (const inner of tool.tools ?? []) {
      if (typeof inner?.name === "string") map.set(inner.name, tool.name);
    }
  }
  return map;
}

/** agent_message 输入项 → 普通用户消息（只给 aimock 看，Codex 收到的流不受影响）。 */
function flattenAgentMessages(body) {
  if (!Array.isArray(body?.input)) return false;
  let changed = false;
  body.input = body.input.map((item) => {
    if (item?.type !== "agent_message") return item;
    changed = true;
    const text = (item.content ?? [])
      .map((part) => part?.text ?? part?.encrypted_content ?? "")
      .join("");
    return { type: "message", role: "user", content: [{ type: "input_text", text }] };
  });
  return changed;
}

/** 给一行 SSE 里的 function_call 项补上 namespace（不认识的行原样返回）。 */
function addNamespaces(line, namespaces) {
  if (!line.startsWith("data: ")) return line;
  let event;
  try {
    event = JSON.parse(line.slice(6));
  } catch {
    return line;
  }
  const patch = (item) => {
    if (item?.type === "function_call" && namespaces.has(item.name)) item.namespace = namespaces.get(item.name);
  };
  patch(event.item);
  for (const item of event.response?.output ?? []) patch(item);
  const newline = line.endsWith("\n") ? "\n" : "";
  return `data: ${JSON.stringify(event)}${newline}`;
}

const front = http.createServer(async (req, res) => {
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  let body = Buffer.concat(chunks);
  let namespaces = new Map();
  const headers = { ...req.headers };
  if (req.method === "POST" && (req.url ?? "").startsWith("/v1/responses")) {
    try {
      const json = JSON.parse(body.toString("utf8"));
      namespaces = namespacedTools(json);
      if (flattenAgentMessages(json)) {
        body = Buffer.from(JSON.stringify(json));
        headers["content-length"] = String(body.length);
      }
    } catch {
      // 不是 JSON 就原样转发，由 aimock 回错误。
    }
  }
  const upstream = http.request(
    { host: "127.0.0.1", port: internalPort, method: req.method, path: req.url, headers },
    (reply) => {
      if (namespaces.size === 0) {
        res.writeHead(reply.statusCode ?? 502, reply.headers);
        reply.pipe(res);
        return;
      }
      const replyHeaders = { ...reply.headers };
      delete replyHeaders["content-length"];
      res.writeHead(reply.statusCode ?? 502, replyHeaders);
      reply.setEncoding("utf8");
      let pending = "";
      reply.on("data", (text) => {
        pending += text;
        let index;
        while ((index = pending.indexOf("\n")) >= 0) {
          res.write(addNamespaces(pending.slice(0, index + 1), namespaces));
          pending = pending.slice(index + 1);
        }
      });
      reply.on("end", () => {
        if (pending) res.write(addNamespaces(pending, namespaces));
        res.end();
      });
    },
  );
  upstream.on("error", (error) => {
    res.writeHead(502, { "Content-Type": "text/plain" });
    res.end(`m6-llm: ${error.message}\n`);
  });
  upstream.end(body);
});
front.listen(port, host, () => console.log(`m6-llm listening on http://${host}:${port}（aimock 在 127.0.0.1:${internalPort}）`));

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, async () => {
    front.close();
    await mock.stop();
    process.exit(0);
  });
}
