# `/status` Diff Summary Design

## Goal

Make each dirty repository in `/status` show a concise, icon-led semantic summary of
the complete diff. Never replace that summary with changed/untracked counts.

## Behavior

- A clean repository displays `✅ Clean`.
- A dirty repository sends its complete redacted status and staged/unstaged diff to
  the configured light model without input truncation.
- The prompt requests one concise content summary beginning with a relevant icon.
- The complete returned summary is displayed without output truncation.
- If a nonempty model response lacks a leading icon, prefix it with `📝`.
- A failed, timed-out, or empty response is retried once.
- If both attempts fail, display `⚠️ Summary unavailable: <error>` instead of git
  status counts.

## Scope

Keep the existing Git section and repository labels. Do not add file lists,
compatibility fallbacks, configuration, or unrelated refactoring.

## Testing

Tests cover clean repositories, complete untruncated diff input, complete
untruncated summary output, icon enforcement, retry success, two-attempt failure,
and the absence of count-only fallback output.
