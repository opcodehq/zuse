# Cleanup ownership and verification

Git history matching and hidden-commit contraction have one implementation in
`zuse_protocol::presentation::git_history`. Both repository RPC filtering and the
history pane use it. The presentation module remains pure: it owns no filesystem,
process, RPC or GPUI state. UI search no longer imports matching behavior from the
engine. Existing engine and UI tests exercise Unicode matching, sparse ancestry,
merges and a 20,000-commit hidden gap through this shared implementation.

The UI cleanup removes unreachable branch popover/footer renderers, their private
geometry and label helpers, unused account setters, and a redundant discard
request wrapper. Whole-source reference checks and compiler diagnostics identified
these paths. Active compact pickers, checkout planning, account dialogs, discard
handling, voice stage and composer voice controls remain. The unused footer orb
is no longer allocated for each shell.

The project-switch test checks the active checkout behavior: stale references
cannot generate actions while the next project's references load. A geometry test
that exercised only an unreachable footer renderer was removed with that renderer.
The folder-search fixture now supplies a path whose trailing components actually
match its query; production folder-ranking behavior is unchanged.

Targeted verification passed 29 repository tests, 34 history UI tests, five voice
stage tests, 13 account tests and two protocol graph-layout tests. The picker suite
passes 52 tests and fails five. A control run with the picker source restored from
checkpoint `c934c556` reproduces the same five failures (53 passing, including the
removed footer-only test): completion prefix length, nested menu mouse targeting,
project ordering, projectless checkout state and provider selection restoration.
Those failures remain unresolved; this cleanup does not claim a green full suite.

Large shell, composer and transcript modules still warrant further focused work.
Moving their code into more files alone would not reduce complexity. Future cuts
should remove duplicate state or make one component own a complete behavior, with
active-flow tests protecting that behavior.
