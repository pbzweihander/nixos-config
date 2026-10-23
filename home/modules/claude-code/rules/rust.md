---
paths:
  - "**/*.rs"
---

# Rust code

- Before finishing, run the repository's CI fmt and clippy commands with the same flags
  and fix every warning. Prefer `#[expect(lint, reason = "...")]` over a bare `#[allow]`.
- Every `unsafe` block has a `// SAFETY:` comment; use a std API over libc when one exists.
- Non-test code: no bare `unwrap()` (match, or `expect("why it holds")`); convert
  untrusted numbers with `try_from`/`checked_*`, not `as`.
- Closed set of variants: an enum, not a trait. An unclear `bool` parameter: an enum.
  Several return values: a named struct. Serialized output: `#[derive(Serialize)]`
  structs, not a `serde_json::Value` indexed back.
- No glob imports except `use super::*` in tests. No `get_` getters.
- Stable rustfmt silently ignores nightly-only options such as `imports_granularity`.
  When rustfmt leaves an expression unformatted, split the long literal in it.
- Apply this to code you write or change, not as a repository-wide sweep. Skip
  `#[non_exhaustive]`, `must_use_candidate`, global `missing_docs` and renaming domain
  abbreviations unless asked. The repository's CLAUDE.md or AGENTS.md wins over this file.
- Source: https://google.github.io/styleguide/rustguide.html (Chromium, Fuchsia and
  library-only rules excluded).
