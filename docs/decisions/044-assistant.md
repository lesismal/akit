# 044: The assistant: fit's assistant in the console, fit's wire, akit's names

[English](044-assistant.md) | [简体中文](044-assistant.zh-CN.md)

Status: accepted in the plan of the track (2026-11-28), waiting for review ([decisions waiting for
review](../decisions-pending.md), rows 98–102); extended by the track's iterations (I160–I169) as
they land.

## Context

fit's I124–I130 and I152–I154 give fit's workbench an assistant
([fit's decision 031](https://github.com/lesismal/fit/blob/main/docs/decisions/031-assistant.md)):
a chat panel beside every page that reads what the running fit does through its Admin API, called
in process, explains it, and drafts changes, either into the open page as an unsaved draft or as a
proposal that a person applies with a click and a second confirm. It calls the model APIs its
settings name; it keeps its settings, the providers' keys, the users' conversations, its skills,
its audit log and its MCP tokens in sections of the encrypted store beside the config; and an MCP
server lets outside agents use its tools. fit's workbench pages, the module `ai` among them, are
in akit already: I91 copied them, and decision 037 renamed what they say of fit. akit's cluster
manager (I132–I139) answers the assistant's level of a user empty, as fit's I153 was left to this
track.

Three things decide how akit follows: where the assistant lives in akit's crates, what of fit's
wire it keeps, and how its model calls meet akit's rules on blocking, bounds and retries.

## Decision

1. **The track is fit's, in ten iterations** (I160–I169, the dev plan's "AI assistant"), in fit's
   order but for two changes: fit's I124 is split into the settings (I160) and the model client
   (I161), and fit's I128 and I129 are joined (I165), as fit's I128 is the pages' own work, which
   I91 copied. fit's I153, the assistant across a cluster, comes last (I169), as it needs the
   cluster track's I139 and, for its metrics, logs and certificates, I139c.
2. **A crate of its own, `akit-assistant`, made by the console.** As fit's console makes its
   assistant (`admin/console.go`), akit-admin's console makes it when `admin.assistant` is set and
   serves it as the console module `ai`, beside the config's own and the roles' (fit's
   `ModuleID`); akit-admin depends on akit-assistant, never the reverse. The assistant reads the
   Admin API in process through a reader akit-admin gives it, the endpoints the workbench's pages
   call, and changes nothing but through a person's apply (I163). The cluster manager meets it
   through an interface of plain types (fit's `assistant.Cluster`), so that neither crate imports
   the other (I169).
3. **fit's wire**: the module's API (`/api/m/ai/...`, its JSON members and errors), the chat's
   Server-Sent Events, the store's section `assistant` (`{"settings", "keys"}`) and its siblings
   (`assistant.chats.<hash>`, `assistant.skills`, `assistant.audit`, `assistant.mcp`), the skills'
   format, and the MCP protocol and its revisions are fit's, so that the pages run unchanged, a
   store moves between akit and fit with its assistant (decision 036, 6, on the `fitstore`
   format), and an MCP client of either reaches both. The names that say fit say akit
   (decision 037): the MCP server's name, its resources' scheme `akit://` (fit's `fit://`), the
   system prompt and the skills naming akit; the config's fields are fit's.
4. **A client of model APIs, never a model.** The eight kinds of fit (`anthropic`, `openai`,
   `openai_responses`, `gemini`, `azure_openai`, `ollama`, `openai_compatible`, `llm_proxy`), each
   request and streamed answer as fit builds and parses them. The calls go out on akio-http's
   asynchronous client (rule 1), on the listeners' engine, as fit's on fib's; nothing waits on an
   I/O thread, the tools run on a pool of the assistant's own, and what reads or writes the store
   runs on it too.
5. **Bounded, with linked backpressure.** Every buffer is fit's bound: a line or event of a model's
   stream 1 MiB, an answer `limits.max_stream_bytes`, 64 tool calls, an error's message 8 KiB, a
   tool's result `max_tool_result_bytes`, the conversations, skills, audit entries and tokens by
   their limits. A chat's stream to the browser is linked to the model's (rule 8, decision row
   102): past a watermark of bytes the browser has not taken, the model's answer is held and resumed
   once they drain, where fit writes on without waiting.
6. **No timer-based retries.** A model call is tried again only at once, only before any of its
   answer was taken, and only when its connection failed or closed (decision row 100); an answer
   of 408, 429 or 5xx fails the call, saying what the API asked (its `Retry-After`), for the person
   to send again. fit waits for `Retry-After`, up to 10 s, and tries again.
7. **Secrets never leave**, as fit's: the keys are kept in the store, written once and shown masked
   (the last four characters of a key of 16 or more), never answered, logged, exported or put in an
   error; a key goes only over https or to a loopback host; every tool result, diff and draft is
   redacted as fit redacts them (akit-admin's `redact`, which already follows fit's
   `assistant/redact.go`), and the members no tool reads are left out.
8. **Off by default.** No `admin.assistant`: no module `ai`, no panel, no outgoing call. Like all
   of `admin`, it takes effect when akit starts; the Settings page's changes to the settings take
   effect at once, kept in the store.
9. **Tests never reach a real model.** Every test talks to a stand-in of the model's API on port 0
   (fit's `fakeapi`, ported), or to akit's own `llm.proxy` with a node whose upstream is such a
   stand-in; no test needs a key or the internet. The evaluation against a real model (fit's
   `make assistant-eval`, I165) runs only when a person names one in the environment.

## Consequences

- akit-admin depends on akit-assistant, which brings the model client into akit-admin's
  dependencies; the binary built without the console's assistant is no smaller for it.
- A store an akit with the assistant wrote is version 3 (the sections), as fit's; an akit or fit
  older than sections cannot open it.
- A user who sends a message while a model's API rate-limits gets the refusal with how long the
  API asked to wait, rather than an answer after a silent wait.
- A slow browser slows the model's stream it reads instead of growing akit's memory; a browser that
  takes nothing for fit's 60 s still ends its chat.
