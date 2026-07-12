# GPT-5.6 Default Models

## Goal

Use Codex-supported GPT-5.6 models for Gateway's built-in primary and light model slots.

## Design

- Set the primary Codex model to `gpt-5.6-sol` and leave its reasoning effort unspecified, so Codex uses its default `medium` effort.
- Set the light Codex model to `gpt-5.6-luna`.
- Force `model_reasoning_effort="low"` whenever Gateway invokes the built-in Luna light model. `low` is the fastest effort exposed for Luna by Codex CLI 0.144.1.
- Update the README model examples to match the built-in defaults.

## Scope

Keep the change limited to default constants, Codex argument construction, documentation, and focused tests. Do not change OpenRouter defaults or add general per-model reasoning configuration.

## Verification

- Assert default configuration contains Sol as the primary model and Luna as the light model.
- Assert Luna command arguments include `model_reasoning_effort="low"`.
- Assert other Codex models do not receive a reasoning override.
- Run formatting and the full test suite.
