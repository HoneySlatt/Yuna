# Changelog fragments

A change that users see adds one file here, rather than editing [`CHANGELOG.md`](../CHANGELOG.md):
two changes made at once then never meet in one file.

- **Its name** is `<section>-<name>.md`. The section is one of `added`, `changed`, `deprecated`,
  `removed`, `fixed`, `security` and `known-limitations`; the name, in lowercase words joined by
  `-`, says what changed: `added-dark-theme.md`.
- **Its text** is the section's entries, each a list's item starting with `- `, written as
  `CHANGELOG.md`'s are: what a user of the project sees, and the ADR that decided it, by its
  number in brackets, `(ADR-0117)`; a change no ADR decided names the ADR of what it changes, or
  none.
- **Several sections** take several files.

`scripts/check.sh` checks every fragment's name and form. At a release,
`scripts/changelog.py --merge` moves every fragment into `CHANGELOG.md`'s `[Unreleased]`, each at
the end of its section, and deletes them.
