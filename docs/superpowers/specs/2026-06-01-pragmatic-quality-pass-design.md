# Pragmatic Quality Pass Design

## Goal

Run a focused code-quality pass that preserves behavior while fixing current
lint blockers, trimming local complexity, improving high-value test coverage,
and producing a concise CRAAP and mutation-readiness audit.

## Scope

The pass is limited to small, behavior-preserving changes in the currently
failing or directly adjacent code paths. It will not restructure the large
modules wholesale, change runtime behavior, add compatibility shims, or chase
100% coverage at any cost.

## Baseline

Initial checks on 2026-06-01:

1. `cargo test` passes with 137 tests.
2. `cargo clippy --all-targets --all-features -- -D warnings` fails on two
   `too_many_arguments` diagnostics:
   1. `src/bot.rs`: `select_model_slot`.
   2. `src/codex.rs`: `run_codex_stream`.
3. `cargo llvm-cov --all-targets --summary-only` reports 93.10% line coverage.
4. `cargo-mutants` is not installed.

## Approach

1. Reduce argument fan-out in `src/bot.rs` by grouping the model-selection
   message/session routing data behind a small local context struct.
2. Reduce argument fan-out in `src/codex.rs` by introducing a request/options
   struct for session, provider, model, timeout, and state directory inputs.
3. Keep the public behavior and command output stable, relying on existing tests
   to detect regressions.
4. Add or update focused tests only where the edited interfaces expose useful
   behavior or where coverage identifies a nearby high-value gap.
5. Use `brew` to install mutation tooling if available, then run a scoped
   mutation pass against edited or high-value files.

## Verification

Required verification commands:

1. `cargo test`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo llvm-cov --all-targets --summary-only`
4. Mutation analysis with `cargo-mutants` if installed through `brew`; otherwise
   report the failed installation or availability check.

## Audit Output

The final response will include:

1. A concise edit summary.
2. Test, clippy, coverage, and mutation results.
3. A CRAAP analysis for the evidence used:
   1. Currency.
   2. Relevance.
   3. Authority.
   4. Accuracy.
   5. Purpose.
4. Any remaining risk or follow-up that is directly tied to observed evidence.
