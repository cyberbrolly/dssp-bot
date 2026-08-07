# Contributing to dssp-bot

Thanks for your interest in improving dssp-bot. This guide covers how to
propose changes and what we expect from a contribution.

By participating, you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## Getting set up

The project targets TypeScript, Chrome Extension Manifest V3, Vite, and Node.js.
The build configuration is not committed yet, so until it lands, contributions
are limited to documentation, planning, and design work under `docs/`.

## Branching and commits

- Branch off `main`. Use a short, descriptive name: `feat/log-submission-queue`,
  `fix/popup-race`, `docs/adr-storage`.
- Keep commits focused. One logical change per commit.
- Write commit subjects in the imperative mood, under 72 characters:
  `Add retry backoff to submission queue`.
- Reference issues in the body where relevant: `Refs #12`.

## Pull requests

Before opening a PR:

1. Rebase on the latest `main`.
2. Run the lint, typecheck, and test commands for the project once they exist.
3. Update `CHANGELOG.md` if the change is user-facing.
4. Update or add documentation for behaviour changes.

In the PR description, explain what changed, why, and how you verified it. Call
out anything you could not test.

## Architecture decisions

Significant or hard-to-reverse technical decisions get an ADR in `docs/adr/`.
Open the ADR as part of the PR that implements the decision, or ahead of it if
you want to settle direction first.

## Code style

Match the surrounding code: its naming, structure, and comment density. Do not
introduce a new library or framework without raising it in an issue or ADR
first.

## Security

Never commit credentials, tokens, cookies, or portal session data — not in code,
fixtures, tests, or logs. If you find a security issue, do not open a public
issue; report it privately to the maintainers.

## Scope

This extension automates a user's own submissions on the DSSP Portal, acting on
explicit user intent. Contributions that bypass authentication, evade rate
limits, scrape other users' data, or otherwise work against the portal's terms
will not be accepted.
