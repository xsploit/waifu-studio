# Agent integration source notes

Inspected 2026-10-17. This is static source evidence, not an authenticated agent acceptance test. No real provider turns, agent logins, Squad joins, private runtime databases, token files, or credential files were used. Local examples below are reference implementations, never runtime dependencies for a public installation.

## Existing local implementation

The mixed Codex/Claude experience is already implemented in `/home/subsect/re-merge/mux`:

- `src/orchestrator.ts`: shared validated schemas for `delegate_task`, `task_status`, `task_cancel`, `agent_send`, task listing/requeue, shared notes, and `claude_agent` / `codex_agent` aliases. Delegation creates visible child threads; `mode=wait` timeout only ends the wait, not the child.
- `src/mux.ts`: supplies dynamic tools on Codex start/resume, dispatches tool calls, enforces root-tree scope, tracks tasks and routes completion into an active or idle parent. T3 clients get a namespaced tool catalog. This is why Claude delegation can work inside Codex without pretending both engines share a native session.
- `src/claude.ts`: real `@anthropic-ai/claude-agent-sdk` `query()` runtime; creates the in-process MCP server `codex` from the same tool schemas, resumes backend sessions, streams native events into Codex-compatible events, applies per-turn policy and pre-tool approval hooks. Claude permission modes are explicitly not an OS sandbox.
- `src/codex.ts`: launches genuine `codex app-server`, brokers JSONL with independent request IDs, bounded buffering and transport error handling.
- `src/policy.ts`, `src/transport.ts`, `src/store.ts`: policy translation, inherited environment and transport ownership, atomic state. Reuse those design patterns; do not copy local routing, launch paths or personal configuration into the product.
- `/home/subsect/.local/bin/squad-worker`: source wrapper demonstrates a persistent receive → task → report loop for Codex or Claude against a shared hub. Its local routing and edit permissions are intentionally not public defaults. No Squad hub was modified. The `squad` executable is a binary; its internals were not established by reading the wrapper.

## Actual installed T3 package

Read the ASAR directory and the specific bundled server member from `/home/subsect/.local/share/t3-custom/squashfs-root/resources/app.asar`; recovered member `/apps/server/dist/binCli-BT6Nd6J0.mjs` is 11,211,072 bytes. This is shipped bundled source, distinct from current GitHub main.

- `OrchestratorMcpDelegateTaskInput` near recovered line 41724: `task`, optional `target`, `title`, `role`, `mode`, `timeoutMs`, stable `clientRequestId`, runtime and interaction modes.
- Result near line 41734 separates `taskId`, `childThreadId`, `childRunId`, `childNodeId`, provider/model, stable summary and context-transfer ID. `workState` is `working`, `waiting_for_children`, or `result_available`; a finished turn with live nested work is not a finished task.
- `OrchestratorToolkit` near line 257477 exposes capabilities, delegate/status/cancel and other app-owned tools. Its descriptions explicitly state default asynchronous delegation, parent wakeup, wait-budget semantics, inherited settings and one distinct idempotency key per review round.
- A terminal task's published result remains stable. Cancelling that task does not interrupt unrelated later runs on its backing child thread.

These are useful contracts for a future persistent adapter. Bundled line numbers refer to the recovered member, not the ASAR container. Static code does not prove the installed app currently executes every path.

## Current public T3 source

Inspected [pingdotgg/t3code](https://github.com/pingdotgg/t3code) at commit `6a6ebb278dc70dd859439a3c28960a7d23a1e645`.

- [LICENSE](https://github.com/pingdotgg/t3code/blob/6a6ebb278dc70dd859439a3c28960a7d23a1e645/LICENSE) is MIT, copyright 2026 T3 Tools Inc. Preserve its notice for copied substantial portions; package/provider dependencies retain their own licenses.
- [ProviderAdapter.ts](https://github.com/pingdotgg/t3code/blob/6a6ebb278dc70dd859439a3c28960a7d23a1e645/packages/provider-core/src/server/ProviderAdapter.ts) defines session open/resume, thread ensure/resume, turn start/steer/interrupt, runtime requests, events and failure types. Turn inputs carry app thread/run/attempt/root-node and native provider identities separately.
- Its normalized events include provider-session/thread/turn updates, nodes, subagents, messages, turn items, command deltas, runtime permission requests, plans and terminal outcomes. Raw command deltas are not persisted; final item output is separate.
- Session runtime supports independently subscribed event streams and thread-specific pending-background-work checks. A shared process must not let one sibling pin an unrelated thread open.
- [ProviderRegistry.ts](https://github.com/pingdotgg/t3code/blob/6a6ebb278dc70dd859439a3c28960a7d23a1e645/apps/server/src/provider/ProviderRegistry.ts), Claude/Codex drivers and [Pi adapter](https://github.com/pingdotgg/t3code/blob/6a6ebb278dc70dd859439a3c28960a7d23a1e645/packages/provider-pi/src/server/adapter.ts) implement provider-specific runtimes rather than a universal chat-completions shim. Pi has a dedicated RPC connection, request timeouts and process cleanup.

## Verified standalone command contracts

Pass each token as an argv element; never interpolate a model-generated command into a shell. Probe installed-version support before offering a preset. Local tool allowlists reduce model capabilities; they do not create an OS sandbox or prevent a CLI from storing its own configuration/authentication state.

Claude Code, from the [official CLI reference](https://code.claude.com/docs/en/cli-reference):

```text
claude --safe-mode --strict-mcp-config --print --permission-mode plan --tools Read,Glob,Grep --no-session-persistence -- PROMPT
```

`--safe-mode` disables customizations while retaining existing authentication; `--strict-mcp-config` excludes unconfigured MCP servers. Verified against installed Claude Code 2.1.310 on 2026-10-25. The previous `--bare` preset ignored OAuth/keychain credentials and therefore broke subscription logins. Model overrides are saved independently for Codex and Claude; a successful local Claude acceptance used `sonnet` because the configured Fable model required extra usage credits. `--tools` controls available tools; `--allowedTools` alone is not an availability restriction. Do not enable permission bypass. `--output-format stream-json` is available for event adapters; streaming examples also require `--verbose`.

Pi, from its [official CLI documentation](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/docs/cli.md) and [argument parser](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/src/cli/args.ts):

```text
pi --print --no-session --no-extensions --no-skills --no-mcp --tools read,grep,find,ls --exclude-tools mcp__* -- PROMPT
```

Pi defaults include shell/edit/write tools and has no built-in plan mode. A plain `--tools` list alone retains MCP reach, so disable MCP and extension discovery explicitly. `--mode json` produces one-shot JSONL events; `--mode rpc` is the persistent protocol route. The parser explicitly supports `--` for literal remaining prompts. The wildcard above is a literal argv value, not shell expansion.

Hermes, from official [CLI parser](https://github.com/NousResearch/hermes-agent/blob/main/hermes_cli/_parser.py) and [query-file reader](https://github.com/NousResearch/hermes-agent/blob/main/hermes_cli/main_query_file.py):

```text
hermes chat --oneshot --quiet --query-file -
```

Write the exact prompt to stdin and close it. `--query-file -` reads stdin literally; `-q PROMPT --oneshot` also supports a one-shot query. `--format stream-json` emits JSONL and implies quiet. `--toolsets` / `-t` selects enabled toolsets. `--yolo` bypasses dangerous-command approvals and is not a suitable default. A complete read-only/no-hook/no-plugin execution contract was not established, so Hermes should initially use a reviewed custom command rather than an asserted read-only first-class preset.

Grok API: [official xAI reference](https://docs.x.ai/docs/api-reference) shows the OpenAI client with `base_url="https://api.x.ai/v1"` and `XAI_API_KEY`. This is API chat connectivity, separate from Grok Build's agent CLI. Users must select an available model and configure their own key; no model availability was tested.

Codex flags are verified separately by the parent implementation against official OpenAI documentation. No authenticated Codex CLI invocation was performed in this research.

## Product boundary

The current preview also has an independent native **conversational tool loop**, off by default. Hikari's selected chat model can call Tavily search/page extraction, explicitly enabled Streamable HTTP MCP tools, and local read-only Codex/Claude adapters, inspect results, and answer within a fixed call budget. This is separate from the reviewed coding-task loop and does not copy T3's orchestrator. WebWaifu4's `server/src/ai/TavilyTools.ts` and `src/lib/grillo/chat-tools.ts` at `fad86c2a8e67960ef1ba843df15855bce0c46f70` were inspected as search/extraction references; its crawl and combined research helpers are not ported. Real Windows production-worker checks passed Tavily search/extraction and both coding adapters on 2026-10-25. Browser automation requires a configured MCP server and its own acceptance test.

The initial public implementation may stage a companion-proposed task, require review of provider/workspace/prompt, run a bounded one-shot process, show its outcome and explicitly return its result to chat. That is useful task dispatch, not T3's persistent session, subagent, approval-broker, steering or nested-task aggregation parity.

A future persistent provider layer should preserve separate task/session/run identities, idempotency, typed events, exact cancellation ownership, bounded queues, stable result provenance, child-work aggregation and user-controlled account configuration. An optional external mux adapter can be added later; it must not hard-code this machine's mux, Squad hub, paths or routing. Hermes, Homunculus and other custom agents need their own versioned execution contracts before claiming native support. Homunculus was referenced but not inspected in this bounded pass.
