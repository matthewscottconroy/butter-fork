# RFCs

Non-trivial design changes to Butterfork go through this lightweight RFC process.

## When to write an RFC

Write an RFC for any change that:

- Alters a public CLI contract (new/changed/removed subcommand, flag, or output schema)
- Changes the on-disk state layout (`~/.butterfork/`)
- Changes the NDJSON event protocol
- Adds or removes a component binary
- Affects the security model or agent permission boundaries
- Introduces a new external dependency visible to end users

Bug fixes, documentation updates, and implementation-only changes inside a single
component don't need an RFC — open a PR directly.

## Process

1. Copy `0000-template.md` to `rfcs/NNNN-short-title.md` (use the next available number).
2. Fill in all sections. Leave `Status: Draft` and `PR: (pending)`.
3. Open a PR. Mark it `RFC` in the title and add the `rfc` label.
4. Discussion happens on the PR. The RFC is open for community comment for at least 7 days.
5. A maintainer of the affected component(s) merges or closes the RFC PR.
   - Merged → `Status: Accepted`. Implementation PRs can now reference the RFC number.
   - Closed → `Status: Rejected` (with the reason captured in a closing comment).
6. After the feature ships, update `Status: Implemented` and add the version.

## Template

```markdown
# RFC NNNN: Title

**Status:** Draft | Accepted | Rejected | Implemented  
**PR:** #NNN  
**Component(s):** bf-forge, bf-build, …  
**Author:** Your Name

## Summary

One paragraph. What is this change and why does it matter?

## Motivation

What problem does this solve? What is the current behavior and why is it insufficient?

## Design

Concrete proposal. Include CLI output examples, schema changes, and event protocol
changes. This section should be detailed enough that a contributor could implement
it without asking questions.

## Drawbacks

Why might we *not* want to do this?

## Alternatives

What other designs were considered? Why was this one chosen?

## Unresolved questions

What's still open? What should be decided during implementation vs. before accepting?
```

## Decision records

When the answer to a proposal is "no", the RFC is closed with a comment explaining
the reasoning. Future contributors should be able to search closed RFCs before
re-proposing something that was already considered and rejected.
