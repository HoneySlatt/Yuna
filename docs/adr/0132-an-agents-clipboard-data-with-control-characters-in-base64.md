# 0132. An agent's clipboard: data with control characters in base64

- Status: Accepted; supersedes in part [0101](0101-an-agents-clipboard-through-mcp.md) (how `clipboard get` gives its data)
- Date: 2026-11-19

## Context

[ADR-0101](0101-an-agents-clipboard-through-mcp.md) gives an agent the data of a seat's clipboard
"as text when it is UTF-8, else in base64", 1 MiB at most. Stage 1's audit found on 2026-11-18 that
UTF-8 made of control characters is escaped as `\u0000` in JSON, six bytes for one, and given twice
in a tool's result: a reply to 1 MiB of them took 13.6 MB, near the 16 MiB of replies that may
wait (Stage 1's audit).

## Decision

Chosen while fixing the audit's finding, as implementation:

- **`clipboard get` gives the data as text when it is UTF-8 and holds no control character but
  tab, newline and carriage return; else in base64.** Carriage return stays text: its JSON escape
  is two bytes, as newline's, and text from Windows ends its lines with it.
- The rest of ADR-0101 stands: the MIME types, 64 KiB by default and 1 MiB at most, `truncated`,
  5 s, `clipboard set` and `clear`.

## Alternatives considered

- **A bound on the escaped size**: keeps such data as text, but an agent then gets less of it than
  it asked, for a reason it cannot see.
- **Base64 always**: one rule, but plain text, the common case, would grow by a third and be
  unreadable in a log.

## Consequences

- A change to `clipboard get`'s contract, announced in the changelog: an agent that read text
  holding control characters now reads base64.
- A reply to 1 MiB of control characters takes about 2.8 MB.

## Validation and reconsideration

Tested by `a_clipboard_reply_stays_well_under_the_replies_that_may_wait` and the unit test on which
characters force base64. Proposed again if an agent needs those characters as text.
