# Production Gateway Core Context Files Design

## Goal

Gateway-spawned Codex conversations always receive production gateway core
context from runtime-editable Markdown files under `$XDG_CONFIG_HOME/gateway`,
without relying on Codex workspace discovery, setup-time generation, or prompt
prepending.

## Scope

This design applies only to Codex processes launched by Gateway:

1. `gateway run`
2. `gateway bot`
3. `gateway heartbeat`

Manual Codex sessions launched outside Gateway are out of scope.

## Core Files

Gateway owns five always-loaded core context files:

1. `AGENTS.md`
2. `IDENTITY.md`
3. `USER.md`
4. `TOOLS.md`
5. `MEMORY.md`

`HEARTBEAT.md` is not core context. Gateway uses it only as the heartbeat prompt
file when `gateway heartbeat` runs.

## File Responsibilities

### `AGENTS.md`

Scope: gateway operating rules, context-loading policy, safety boundaries,
instruction precedence, and ownership rules for the other core files.

This file answers: how should the assistant operate through this gateway?

### `IDENTITY.md`

Scope: assistant identity only.

This file answers: who is the assistant in this gateway?

Appropriate content includes assistant name, stable persona, presentation style,
and other identity facts that are not user preferences.

### `USER.md`

Scope: user identity, preferences, language, communication style, and shorthands.

This file answers: who is the user and how should the assistant communicate with
them?

### `TOOLS.md`

Scope: local environment facts, tool availability, command conventions, service
endpoints, and operational recipes.

This file answers: what can Gateway use locally, and how should those tools be
used?

### `MEMORY.md`

Scope: durable learned facts and standing instructions that do not belong in a
more specific file.

This file answers: what should survive across gateway conversations?

`MEMORY.md` must not duplicate tool facts, user preferences, assistant identity,
or operating rules that belong in the narrower files.

### `HEARTBEAT.md`

Scope: scheduled heartbeat protocol only.

This file answers: what should `gateway heartbeat` check, maintain, and report?

It is loaded as the heartbeat prompt, not as always-on core context.

## Read Order

Gateway builds developer instructions in this order:

1. Built-in `SYSTEM.md`
2. `AGENTS.md`
3. `IDENTITY.md`
4. `USER.md`
5. `TOOLS.md`
6. `MEMORY.md`

The order moves from global runtime constraints to operating rules, identity,
user preferences, environment details, and finally durable memory. Later files
can clarify more specific facts, but they should not redefine broader safety or
runtime constraints.

## Templates And Runtime Files

Template skeletons live in the repository under `src/prompts/`.

Runtime files live under `$XDG_CONFIG_HOME/gateway/`.

Each template contains only:

1. The Markdown title line.
2. The `> **Scope:** ...` line.

Gateway-created runtime files begin with those two lines. Users and runtime work
may add content below them.

## Runtime Behavior

At the start of each Gateway entrypoint that may launch Codex, Gateway calls an
ensure function once:

1. `gateway run`
2. `gateway bot`
3. `gateway heartbeat`

The ensure function:

1. Creates missing core files from templates.
2. Creates missing `HEARTBEAT.md` from its template.
3. Refreshes only the title and scope lines in existing files.
4. Preserves all user/runtime content below the title and scope lines.

Before each Codex spawn, Gateway:

1. Reads the five core files directly from `$XDG_CONFIG_HOME/gateway`.
2. Combines them with built-in `SYSTEM.md`.
3. Enforces a conservative size limit on the final developer instructions.
4. Passes the result as `-c developer_instructions=...`.
5. Fails closed if any core file cannot be read or the combined context is too
   large.

Gateway does not cache core file contents. Reading five small local Markdown
files is negligible compared with starting Codex and calling the model.

Gateway does not prepend core context to the user prompt. User prompts remain
task input only.

## Heartbeat Behavior

`gateway heartbeat` uses the same core context injection as other Gateway Codex
spawns.

In addition, heartbeat ensures and passes
`$XDG_CONFIG_HOME/gateway/HEARTBEAT.md` as the user prompt file for the heartbeat
task.

Normal Gateway conversations do not load `HEARTBEAT.md`.

## Error Handling

Gateway should fail closed when the context contract cannot be honored:

1. Missing template embedded in the binary.
2. Runtime file cannot be created, refreshed, or read.
3. Combined developer instructions exceed the configured size limit.
4. Codex CLI rejects the generated configuration argument.

Errors should name the affected file and the safest remediation, such as
trimming `$XDG_CONFIG_HOME/gateway/MEMORY.md`.

## Testing Strategy

Tests should cover:

1. Missing runtime files are created from templates.
2. Existing runtime files keep content below the title and scope lines.
3. Header and scope lines are refreshed when templates change.
4. Core context includes exactly `AGENTS.md`, `IDENTITY.md`, `USER.md`,
   `TOOLS.md`, and `MEMORY.md`.
5. Normal Codex spawns do not include `HEARTBEAT.md`.
6. Heartbeat passes `HEARTBEAT.md` as the heartbeat prompt file.
7. Oversized combined context fails before launching Codex.
8. Codex argument construction includes the combined developer instructions.

## Non-Goals

1. Loading these files for manual Codex sessions outside Gateway.
2. Adding a public `gateway context init` or `gateway setup` command.
3. Running context generation from `./setup`.
4. Caching context file contents.
5. Using Codex `AGENTS.md` workspace discovery as the guarantee.
6. Passing core context as user prompt text.
7. Creating compatibility aliases or fallback file names.
