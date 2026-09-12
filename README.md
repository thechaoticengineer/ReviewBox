# ReviewBox

A keyboard-first terminal review inbox for commits across GitHub projects.

See [PRODUCT.md](PRODUCT.md) for the accepted requirements and delivery order.

## Current status: asynchronous daily GitHub inbox

Implemented now:

- A runnable Rust terminal application with repository, commit, file, and
  diff-like panes populated from deterministic fictional fixtures.
- Neovim-style normal-mode focus, selection, scrolling, open/back, search, and
  contextual help behavior.
- Bounded selections and scrolling, resize-aware redraws, and a compact fallback
  for terminals smaller than 60 columns by 16 rows.
- Structured terminal setup and restoration on normal return, propagated
  application errors, and recoverable partial setup failures.
- An executable, noninteractive smoke workflow rendered through Ratatui's
  in-memory test backend.
- Owned inbox, repository, commit, author, file, and diff models shared by the
  fictional demo and future live data. Commits retain their complete SHA,
  subject, GitHub login, and author timestamp; the UI displays only a short SHA.
- Live launch configuration: no arguments selects today's calendar day in the
  detected local IANA timezone. `--date YYYY-MM-DD` and `--timezone IANA_NAME`
  override those values before terminal setup. If local-zone detection is not
  available, the deterministic fallback is `Etc/UTC`.
- A selected day is the half-open interval from local midnight through (but not
  including) the following local midnight. Both boundaries are converted to UTC
  independently, so daylight-saving days can be 23 or 25 hours.
- A read-only, UI-independent GitHub loader that invokes `gh api` directly and
  reuses the GitHub CLI's existing authentication without requesting, reading,
  printing, or persisting a token.
- Explicitly paginated owned-repository, branch, and per-branch commit loading
  at 100 items per page. Dynamic repository path segments are percent-encoded,
  query values remain separate process arguments, and every request explicitly
  uses `GET`.
- Defensive local author and half-open timestamp filtering, full-SHA
  deduplication within each repository, and stable repository, branch, and
  newest-first commit ordering.
- Typed incremental progress, repository snapshots, sanitized scoped failures,
  and terminal complete/incomplete/fatal outcomes. Successful branchless and
  empty repositories remain distinct from inaccessible enumeration; safe
  repository- and branch-scoped failures preserve partial results.
- Asynchronous live loading through a bounded event channel. Terminal input,
  resize handling, search, help, redraws, and quitting remain responsive while
  `gh` requests run. Repository snapshots and deduplicated commits become
  navigable as they arrive, with selection preserved by repository identity and
  full commit SHA when updates reorder data.
- A live status view showing the selected ISO date and IANA timezone,
  repository/branch/commit progress, bounded sanitized errors, and distinct
  complete, empty-day, no-owned-repository, incomplete-coverage, authentication,
  missing-`gh`, permission, and rate-limit outcomes.
- Cancellation on quit. ReviewBox stops consuming updates, signals the loader,
  and kills and reaps an active `gh` subprocess before terminal teardown.

Planned for later delivery increments, and not implemented yet:

- Durable review state and reviewed/unreviewed filtering.
- Real diffs loaded from GitHub repositories.
- Drafting, editing, persistence, and publishing of comments.
- Repeated-match navigation with `n` / `N`.

## Launch configuration and demo

Install Rust 1.88 or newer (including Cargo) and use a terminal that supports
Crossterm. Launch the interactive demo with:

```sh
cargo run -- --demo
```

The normal launch path currently configures the live inbox selection:

```sh
cargo run -- --date 2026-09-12 --timezone Europe/Warsaw
```

Both options are optional; `cargo run` uses today in the detected local IANA
timezone. Argument validation and local-zone fallback happen before the terminal
is changed. The live inbox starts immediately in the background. It requires the
`gh` executable and an existing authenticated GitHub CLI session (normally set
up with `gh auth login`). Every request is a read-only `gh api --method GET`
request. Press `q` or `Ctrl-c` at any time to cancel loading and exit.

The demo needs no GitHub authentication. It performs no network requests or
network writes and does not persist runtime state. All repository names, commit
IDs, files, and diff-like content are fictional.

Run the same fixture/state/rendering integration noninteractively with:

```sh
cargo run -- --demo-smoke
```

The smoke command does not initialize a real terminal or event reader. It
renders representative frames in memory, exercises navigation, search, help,
resize, and clean exit transitions, prints a short success message, and exits
nonzero if an invariant or render operation fails. It also needs no credentials,
network, or persistent storage.

## Implemented modes and keybindings

Normal mode is active at launch. Search entry and help are isolated modes:
normal navigation keys do not navigate while either overlay is active. The
status line shows the mode, focused pane, a pending `g` prefix, and the latest
action.

Normal mode:

- `h` / `l`: focus the previous / next meaningful pane.
- `j` / `k`: move the selection in a list, or scroll the focused diff.
- `gg` / `G`: move to the first / last position in the focused pane. A lone
  `g` waits for one more `g`; any unrelated key safely cancels the prefix before
  performing its own action.
- `Ctrl-d` / `Ctrl-u`: move down / up by half of the focused pane's usable
  height, with a minimum movement of one.
- `Enter`: descend from repository to commit to file to diff when a child is
  available.
- `Escape`: return to the parent pane. At the repository pane it stays put and
  never quits unexpectedly.
- `/`: enter search-entry mode for the focused pane. Typed characters—including
  normal-mode navigation letters—edit the query. `Backspace` edits, `Enter`
  selects or scrolls to the first case-insensitive match with one wrap, and
  `Escape` cancels. Empty and no-match searches leave the current position
  unchanged and report their result in the status line.
- `?`: open contextual keyboard help. `Escape` closes it and restores the prior
  pane focus.
- `q`: quit from normal mode and restore the terminal.
- `Ctrl-c`: quit globally, including from search or help, and restore the
  terminal.

Search mode:

- Printable characters, including normal-mode binding characters, append to the
  query; `Backspace` removes the last character.
- `Enter` searches the focused pane case-insensitively from the current position
  with one wrap, then returns to normal mode. A match selects or scrolls to its
  first occurrence. Empty and no-match searches keep the previous position.
- `Escape` cancels the query and returns to normal mode without moving.

Help mode:

- `Escape` closes help and restores the pane that was focused when help opened.
- Other keys are ignored, except global `Ctrl-c`.

The status line shows the active mode, a pending `g`, the focused pane, and the
most recent action. Search and help remain usable after a resize and fall back
to clipped, panic-free overlays in very small terminals.

## GitHub loading behavior and limitations

The loader first resolves the authenticated account with `GET /user`, then
explicitly paginates `GET /user/repos` with `affiliation=owner` and defensively
checks each returned owner login. For every stably sorted repository it paginates
the branch list and requests commits once per distinct branch with the selected
UTC `since`/`until` interval and authenticated `author` login. Results are checked
again locally against the top-level GitHub `author.login` and
`commit.author.date`; the end instant is always excluded even if the server
returns its inclusive boundary. Shared commits reachable from several branches
are retained once by full SHA.

This is branch coverage, not a transactional snapshot. Commits reachable only
from deleted or inaccessible refs cannot be found, and repositories or branches
can change during traversal. Unlinked commits whose top-level GitHub author is
null are excluded even when commit metadata names or email addresses resemble
the user; ReviewBox does not guess identity from those fields. The filtered time
is Git's documented author timestamp (`commit.author.date`), not committer time,
push time, or the contribution-calendar date. Forks owned by another account are
excluded by the owner-only inbox definition.

GitHub can return permission/not-found, authentication, rate-limit, and other
API failures. A 403 is considered rate-limited only when response headers report
no remaining requests or a retry interval; otherwise it remains a permission
failure. Repository- and branch-scoped errors keep already loaded data but mark
coverage incomplete. Discovery errors are fatal. Diagnostics contain only a
category, numeric scope, and optional HTTP status—not response bodies, stderr,
repository names, branches, SHAs, or subjects.

The terminal displays at most three sanitized failure details and reports how
many more were omitted. An incomplete result is never labeled complete or as a
truly empty day. Live navigation intentionally stops at the commit pane in this
increment; file and diff retrieval remain planned. `--demo` retains the complete
fictional repository/commit/file/diff experience.

Implementation assumptions were checked against the official GitHub
documentation for the [authenticated user](https://docs.github.com/en/rest/users/users#get-the-authenticated-user),
[authenticated-user repositories](https://docs.github.com/en/rest/repos/repos#list-repositories-for-the-authenticated-user),
[branches](https://docs.github.com/en/rest/branches/branches#list-branches),
[commits](https://docs.github.com/en/rest/commits/commits#list-commits),
[pagination](https://docs.github.com/en/rest/using-the-rest-api/using-pagination-in-the-rest-api),
and [rate limits](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api),
plus the official [`gh api` manual](https://cli.github.com/manual/gh_api).

## Terminal restoration

On a normal quit or a propagated application error, ReviewBox cancels active
live loading, restores the cursor, leaves the alternate screen, and disables raw
mode in reverse setup order. Setup tracks acquired resources so a recoverable
partial setup failure also restores what was already changed. Process aborts,
`SIGKILL`, power loss, and equivalent failures cannot be guaranteed to run
cleanup.

## Development checks

Run the complete terminal-foundation verification set with:

```sh
cargo fmt --check
cargo build
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
cargo run -- --demo-smoke
```

Generated build output (`target/`), local runtime/review state, environment
credential files, logs, and editor metadata are ignored. Do not add credentials,
personal repository data, or comment drafts to Git.

## Goal

Choose a day and quickly review your commits across all your GitHub repositories
in one place, without opening each project separately.

## Related project

[CommitPulse](https://github.com/thechaoticengineer/CommitPulse) is a planned
Omarchy widget for daily, weekly, monthly, and yearly GitHub contribution counts.

## License

[MIT](LICENSE) © 2026 thechaoticengineer.
