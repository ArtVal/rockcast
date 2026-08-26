# RockCast agent instructions

## Graphify

Use Graphify only when the user explicitly invokes `/graphify` or when a genuinely large,
cross-repository architecture investigation needs relationship traversal that `rg` and targeted file
reading cannot provide. For routine code search, roadmap/status questions, concrete bugs, and
single-repository work, use `rg` and read only the relevant files.

Do not run `graphify update .` after ordinary changes. Never use Graphify merely because
`graphify-out/` exists or is dirty.

After every Graphify invocation, including failure, timeout, or interruption, inspect Python
processes created by that invocation. Gracefully stop only processes positively identified as
Graphify-owned and verify they exited. Never terminate unrelated Python processes.
