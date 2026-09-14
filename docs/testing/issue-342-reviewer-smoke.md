# Compiler reviewer UI smoke (temporary)

Candidate: `target/debug/horizon` from this PR. Use `--config <temp-config> --ephemeral`.

## Baseline

1. Launch a browser panel. Teach is absent until started.

## Review flow

1. Start **Teach routine**, click one labeled control, **Stop**.
2. Enter a **Heading** completion assertion (the page-title checkbox only prefills that heading; it is not a title assertion).
3. The **Review plan** section lists compiled steps: action, mutation class, resume policy, MCP tool or `no MCP`.
4. Check **Identities reviewed**.
5. **Save routine** persists a named routine under the private registry. A second save of the same session overwrites the same UUID.

## Errors

1. Stop with no outcome and empty heading: review shows an assertion error, Save fails, banner keeps Teach stopped.
2. Discard still removes the banner without leaving a saved routine if Save was not used.

Delete this file after the validation pass unless asked to keep it.
