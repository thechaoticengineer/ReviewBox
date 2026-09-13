# ReviewBox

A keyboard-first terminal review inbox for commits across GitHub projects.

See [PRODUCT.md](PRODUCT.md) for the accepted requirements and delivery order.

## Current status: diff review and commit comments

Implemented now:

- A runnable Rust terminal application with repository, commit, file, and diff
  panes populated from deterministic fictional fixtures or live GitHub data.
- Neovim-style normal-mode focus, selection, scrolling, open/back, repeated
  search, and contextual help behavior.
- Bounded selections and scrolling, resize-aware redraws, and a compact fallback
  for terminals smaller than 60 columns by 16 rows.
- Structured terminal setup and restoration on normal return, propagated
  application errors, and recoverable partial setup failures.
- An executable, noninteractive smoke workflow rendered through Ratatui's
  in-memory test backend.
- Owned inbox, repository, commit, author, file, and typed diff models shared by
  the fictional demo and live data. Repositories retain GitHub's numeric ID as
  their stable identity. Commits retain their complete SHA, subject, GitHub
  login, and author timestamp; the UI displays only a short SHA.
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
- A read-only, on-demand commit-detail loader for one selected repository and
  complete SHA. It preserves API file order and metadata, parses text patches
  into typed, numbered lines, and explicitly represents empty, unavailable,
  locally capped, response-truncated, loading, and sanitized failure outcomes.
- Commit-detail retention is bounded to 300 files, 256 KiB and 5,000 lines per
  file, 2,000 characters per line, and 2 MiB of patch text per commit. A
  response over the existing 16 MiB command-output limit is reported as
  truncated rather than malformed. File paths and patch lines have terminal
  control characters replaced, tabs expanded, and trailing carriage returns
  removed.
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
- Live commits open into file and diff panes through an asynchronous, on-demand
  detail request. Repeated opens share the active or cached result, changing the
  selection rejects stale results, and a 16-commit session cache bounds retained
  details. Loading, request failure, response truncation, unavailable/binary
  patches, empty patches, locally capped patches, and incomplete file lists are
  shown explicitly.
- Reviewed marks are active in the live UI. The review-progress store records only
  GitHub numeric repository IDs and complete lowercase commit SHAs in a
  deterministic, versioned JSON file at
  `$XDG_DATA_HOME/reviewbox/review-state.json`, or
  `$HOME/.local/share/reviewbox/review-state.json` when `XDG_DATA_HOME` is unset,
  empty, or not absolute. Marks load before the terminal UI, remain visible in
  the unfiltered inbox, and a failed save leaves the displayed mark unchanged
  with an error. The remaining-only view hides reviewed commits and repositories
  without remaining work and reports an explicit all-reviewed state.
- A shared live/demo unified-diff viewer with old/new-number gutters,
  syntax-aware hunk/addition/deletion styling, Unicode-safe soft wrapping, and
  scroll behavior that reaches the actual tail of long patches after resize.
  It explicitly labels no textual changes, binary/API-omitted patches, local
  per-file and per-commit caps, capped file lists, oversized GitHub responses,
  and sanitized detail failures.
- `/` saves its query for `n`/`N` repeat navigation in the currently focused
  repository, commit, file, or diff pane. Repeats are case-insensitive, wrap in
  either direction, keep list matches visible, and highlight/scroll to diff
  matches. Search-entry keys remain isolated from normal-mode bindings.
- A durable comment-draft persistence layer is initialized in live mode and kept
  separate from reviewed progress. A keyboard-only modal editor creates and
  changes commit-wide drafts from the Commit and File panes and supported line
  drafts at the selected Diff row. Draft and diff-cursor markers, target and
  eligibility feedback, multiline cursor editing, explicit save/cancel behavior,
  sanitized save errors, and full/compact EDIT layouts are implemented.
- Commit and supported line drafts can be opened in the first nonblank configured
  `$VISUAL`, then `$EDITOR`, including commands such as `nvim -f`. ReviewBox
  restores the normal terminal before launch and reacquires, clears, resizes, and
  redraws the TUI afterward. Successful nonblank edits are saved; launch, exit,
  read, UTF-8, size, or save failures preserve the prior durable draft.
- Existing commit comments load on demand in a keyboard-scrollable overlay.
  Publishing is a separate `P` action with a `y` confirmation and uses only
  GitHub's commit-comments endpoint. Created comments refresh the overlay;
  failures preserve the draft, and ambiguous outcomes remain durably locked
  until marker-based reconciliation proves whether the exact attempt exists.

Planned for later delivery increments, and not implemented yet:

- Installation packaging and final workflow polish.

## Launch configuration and demo

Install Rust 1.88 or newer (including Cargo) and use a terminal that supports
Crossterm. Launch the live inbox for today in the detected local timezone with:

```sh
cargo run
```

Select a different date or IANA timezone with either or both options:

```sh
cargo run -- --date 2026-09-12 --timezone Europe/Warsaw
```

Launch the interactive fictional demo with:

```sh
cargo run -- --demo
```

Both options are optional and independent. Without `--date`, ReviewBox chooses
today as observed in the selected timezone; without `--timezone`, it uses the
detected local IANA timezone. If detection fails or produces an invalid IANA
name, it visibly falls back to `Etc/UTC`. Argument validation and timezone
fallback happen before the terminal is changed.

The live inbox starts loading immediately in the background. It requires the
`gh` executable and an existing authenticated GitHub CLI session, normally set
up with [`gh auth login`](https://cli.github.com/manual/gh_auth_login). The
credential must be allowed to see each desired repository: GitHub documents
Metadata (read) permission for authenticated repository discovery and Contents
(read) permission for listing private-repository branches and commits. A
classic token needs the `repo` scope to include private repositories; a
fine-grained token must select the desired repositories and grant those read
permissions. Repositories hidden from the credential cannot be discovered.
ReviewBox does not inspect or persist the credential. Inbox, detail, and comment
loading use explicit `gh api --method GET` requests. The only write request is
the user-confirmed commit-comment `POST` described below. Press `q` or `Ctrl-c`
at any time to cancel active work and exit.

The demo needs no GitHub authentication. It performs no network requests or
network writes and does not persist runtime state; reviewed marks and comment
draft state exist only in memory for that process. It may launch an explicitly
configured external editor when `E` or `Ctrl-e` is pressed, but that path remains
network-free. All repository names, commit IDs, files, and diff-like content are
fictional.

Run the same fixture/state/rendering integration noninteractively with:

```sh
cargo run -- --demo-smoke
```

The smoke command does not initialize a real terminal or event reader. It uses
the in-memory review store and renders representative frames for repository,
commit, file, and long-diff navigation; forward/backward wrapped search;
reviewed toggling and remaining filtering; binary, capped, and oversized
responses; help; resize; and clean exit. It prints a short success message and
exits nonzero if an invariant or render operation fails. It needs no credentials
or network and does not write persistent state.

## Implemented modes and keybindings

Normal mode is active at launch. Search entry and help are isolated modes:
normal navigation keys do not navigate while either overlay is active. The
status line shows the mode, focused pane, a pending `g` prefix, and the latest
action.

| Mode | Keys | Action |
| --- | --- | --- |
| Normal | `h` / `l` | Focus the previous / next meaningful pane. |
| Normal | `j` / `k` | Move a list selection or scroll the focused diff down / up. |
| Normal | `gg` / `G` | Move to the first / last position. A lone `g` waits for a second `g`; another key cancels the prefix, then performs its normal action. |
| Normal | `Ctrl-d` / `Ctrl-u` | Move down / up by half the focused pane's usable height, with a minimum movement of one. |
| Normal | `Enter` | Open repository → commit → file → diff. Opening a live commit starts an asynchronous detail request; `Enter` retries a failed request. |
| Normal | `Escape` | Return to the parent pane. At Repository it stays put. |
| Normal | `/` | Enter search for the focused pane. |
| Normal | `n` / `N` | Repeat the saved case-insensitive search forward / backward, wrapping at either end. |
| Normal | `m` | Toggle the selected commit reviewed/unreviewed from Commit, File, or Diff. A live mark changes only after a successful save. |
| Normal | `f` | Toggle all commits / remaining commits. This filter is not persisted. |
| Normal | `c` | Create or edit the selected commit draft from Commit or File, or the selected supported line draft from Diff. Repository and unsupported diff rows show a refusal. |
| Normal | `E` | Open that same contextual draft in `$VISUAL` or `$EDITOR`. A missing draft starts with an empty buffer; Repository and unsupported diff rows show a refusal. |
| Normal | `P` | Publish that saved contextual draft. A confirmation overlay accepts only `y`; every other key cancels. An unresolved prior attempt reconciles instead of posting again. |
| Normal | `C` | Open existing comments for the selected commit. This also reconciles an unresolved publish attempt for that commit. |
| Normal | `?` | Open contextual keyboard help. |
| Normal | `q` | Quit and restore the terminal. |
| Search | Printable characters | Append text, including characters that are navigation keys in Normal mode. |
| Search | `Backspace` | Remove the last query character. |
| Search | `Enter` | Apply the query in the focused pane and return to Normal mode. The query remains available to `n` / `N`, even after no match. |
| Search | `Escape` | Cancel without moving and return to Normal mode. |
| Help | `Escape` | Close help and restore its prior focus. Other non-global keys are ignored. |
| Comments | `j` / `k`, `r`, `Escape` | Scroll, refresh, or close the existing-comments overlay. |
| Edit | Printable characters / `Enter` | Insert text or a newline. Normal-mode keys, including `h`, `j`, `k`, `l`, `q`, `n`, `/`, `?`, `m`, `f`, `g`, `G`, `c`, `E`, `P`, and `C`, are inserted literally. |
| Edit | `Backspace` | Delete the character before the cursor. |
| Edit | Arrow keys / `Home` / `End` | Move by character, line, or to the current line boundary in multiline text. |
| Edit | `Escape` | Save and return to the prior pane. Whitespace-only text deletes an existing draft or discards a new one. A failed save keeps the text open for retry. |
| Edit | `Ctrl-g` | Cancel changes, restore the last saved body, and return to the prior pane. |
| Edit | `Ctrl-e` | Open the current in-memory buffer externally, save a successful replacement, and stay in Edit. If saving fails, the replacement remains in the Edit buffer for recovery. |
| Global | `Ctrl-c` | Quit from Normal, Search, or Help. In Edit, save first and quit only if that succeeds. |

Search starts after the current list selection or diff position, selects or
scrolls to the first match, and wraps once. Empty and no-match searches do not
move. Results, missing prior queries, and wraparound are reported in the status
line. Search text is retained only for the current process.

The status line shows the active mode, a pending `g`, the focused pane, and the
most recent action. Search and help remain usable after a resize and fall back
to clipped, panic-free overlays in very small terminals.

## Reviewed-state storage and privacy

Live mode reads reviewed progress from exactly one XDG user-data file:

- If `XDG_DATA_HOME` is an absolute path:
  `$XDG_DATA_HOME/reviewbox/review-state.json`.
- Otherwise, if `HOME` is an absolute path:
  `$HOME/.local/share/reviewbox/review-state.json`.
- If neither location is usable, browsing continues with a warning and review
  marks are disabled rather than written relative to the working directory or a
  repository.

Each mark is keyed by GitHub's numeric repository ID and the complete commit SHA,
not by owner/name, repository position, abbreviated SHA, subject, or selected
day. A repository rename therefore does not invalidate progress. The versioned
JSON contains no tokens, repository names, owners, branches, subjects, file
paths, patches, comment drafts, or other repository content.

Each toggle re-reads the current file, applies one mark change, and atomically
replaces the file from a same-directory temporary file. This narrows, but does
not eliminate, a last-writer race if two ReviewBox instances save at nearly the
same time; no inter-process file lock is used. Missing state is treated as empty.
A malformed file, unsupported version, permission error, or read error leaves
live browsing available, displays a persistent sanitized warning, disables mark
changes for that run, and is never silently overwritten. Save failures leave the
visible mark unchanged. The `f` filter itself is process-local and always starts
in the all-commits view; only reviewed marks survive restart. Demo and demo-smoke
use memory-only marks and never construct the file-backed store.

## Comment-draft storage and targeting

Live mode reads comment drafts from a separate versioned file:

- If `XDG_DATA_HOME` is an absolute path:
  `$XDG_DATA_HOME/reviewbox/comment-drafts.json`.
- Otherwise, if `HOME` is an absolute path:
  `$HOME/.local/share/reviewbox/comment-drafts.json`.
- If neither location is usable, browsing continues with a sanitized warning and
  file-backed drafts are disabled rather than written relative to the repository.

Draft identities use GitHub's numeric repository ID, the complete lowercase
commit SHA, and either the whole commit or a line target made from the API file
path and GitHub commit-diff position. Bodies must contain non-whitespace text and
are limited to 65,000 characters; line paths are limited to 4,096 bytes and may
not contain control characters. The file is capped at 16 MiB when read.

Press `c` in the Commit or File pane to edit the selected commit-wide draft.
In the Diff pane, `j`, `k`, `gg`, `G`, `Ctrl-d`, and `Ctrl-u` move a highlighted
logical-row cursor; `n` and `N` move it to diff search matches. The Diff title
shows whether that row is commentable and, when it is, its path and GitHub patch
position. Press `c` there to edit that exact line target. Commit rows and diff
gutters display a `◆` for saved drafts. The editor identifies its target and
whether the draft is new or saved, wraps text, and exposes the terminal cursor
in both full and compact layouts.

Only retained context, addition, and deletion rows in a textual patch beginning
with a valid `@@` hunk header can become line targets. Hunk headers, no-newline
markers, malformed/other rows, binary or unavailable patches, and content omitted
by the commit-wide budget are ineligible. Rows retained by the per-file cap remain
eligible. A filename changed by terminal sanitization is also ineligible, because
the displayed path would not safely identify the API target. Positions count raw
patch rows from the first hunk header, including intervening headers and notices,
as required by GitHub's commit-comments API.

Every file update re-reads the existing draft file, validates it, changes one
target, and atomically replaces it through a same-directory private temporary
file. The `reviewbox` directory is created with mode `0700` and the replacement
file with mode `0600` on Unix. Deterministic ordering and a trailing newline make
unchanged rewrites byte-identical. A malformed, unsupported, oversized, or
unreadable file is reported and never overwritten; failed replacement preserves
the previous bytes. No lock coordinates concurrent ReviewBox processes, so two
simultaneous writers can still race. Drafts are never stored in
`review-state.json`. Demo and demo-smoke construct only memory-backed draft state.

External editing selects the first nonblank value from `$VISUAL`, then `$EDITOR`.
The value is split into an executable and arguments with quote and backslash
support, but is never passed through a shell; the private draft body and temporary
path are not interpolated into a command string. The temporary `draft.md` is
created outside the repository under an absolute `$XDG_RUNTIME_DIR`, when set,
or the system temporary directory. Its new directory uses mode `0700` and its
file mode `0600` on Unix. Input and output are capped at 1 MiB and decoded as
strict UTF-8, with the durable 65,000-character draft limit enforced. The file
and directory are removed best-effort after every launch path. A successful
blank result or a nonzero editor exit leaves the previous draft unchanged.

Publishing sends `POST /repos/{owner}/{repo}/commits/{full_sha}/comments` with
the body alone for a commit draft, or body, path, and validated GitHub diff
position for a line draft. ReviewBox does not use pull-request review or issue
comment APIs. Before posting, it appends a unique HTML marker and atomically
stores that attempt and its start time in the draft file. A parsed `201 Created`
response removes the draft. Definite client/authentication failures clear only
the attempt lock; transport failures, cancellation, unexpected statuses, and
unreadable success responses keep it locked. `P` or `C` then lists comments and
searches the raw API body for that exact marker before any display truncation.
Only a match removes the draft. A complete not-found listing may unlock a retry
after 60 seconds; an incomplete or younger result never does. Editing and repeat
publishing stay disabled while the outcome is unresolved.

Comment lists fetch up to ten pages of 100 comments and label capped results as
incomplete. Displayed author, path, timestamp, and body text are sanitized, and
bodies are capped at 8,000 characters. The demo and demo-smoke flows use an
in-process fake comment service with no `gh` or network-write path.

## GitHub loading behavior and limitations

The loader first resolves the authenticated account with `GET /user`, then
explicitly paginates `GET /user/repos` with `affiliation=owner` and defensively
checks each returned owner login. It therefore includes accessible public and
private repositories owned by that personal account, but not organization-owned
repositories or repositories owned by someone else where the user collaborates.
Pages contain at most 100 items and are fetched until a short or empty page.

For every stably sorted repository, ReviewBox paginates every branch currently
returned by GitHub and requests commit pages once per distinct branch, also 100
at a time until a short or empty page. Each request supplies the branch, the
authenticated `author` login, and the selected UTC `since` and `until`
boundaries. The correctness interval is exactly
`[selected-date 00:00:00, following-date 00:00:00)` in the named IANA timezone;
its two local midnights are independently converted to UTC. Results are checked
again locally against an exact, case-insensitive top-level GitHub `author.login`
match and the Git author timestamp in `commit.author.date`. The start instant is
included and the end instant excluded. Shared commits reachable from several
branches are retained once per repository by full SHA.

This is branch coverage, not a transactional snapshot. Commits unreachable from
the currently listed branch heads—including tag-only, dangling, deleted-ref, or
inaccessible-ref commits—cannot be found. Inaccessible repositories and branches
are also absent or incomplete, and repositories or branches can be created,
deleted, or advanced during traversal. Unlinked commits whose top-level GitHub
author is null are excluded even when commit metadata names or email addresses
resemble the user; ReviewBox does not guess identity from those fields. The
filtered time is Git's author timestamp (`commit.author.date`), not committer
time, push time, or the contribution-calendar date. Forks owned by another
account are excluded by the owner-only inbox definition.

GitHub can return permission/not-found, authentication, rate-limit, offline, and
other API failures. A 403 is considered rate-limited only when response headers
report no remaining requests or a retry interval; otherwise it remains a
permission failure. Repository- and branch-scoped errors keep already loaded
data but mark coverage incomplete. Discovery errors are fatal. Diagnostics
contain only a category, numeric scope, and optional HTTP status—not response
bodies, stderr, repository names, branches, SHAs, or subjects.

The terminal displays at most three sanitized failure details and reports how
many more were omitted. An incomplete result is never labeled complete or as a
truly empty day. Live and demo modes use the same four-pane
repository/commit/file/diff workflow and typed patch model. Commit details are
requested only when a live commit is explicitly opened; file lists beyond 300
entries and command responses beyond 16 MiB are disclosed as incomplete rather
than presented as complete.

GitHub's commit response can omit a textual `patch`. When it also reports zero
changes, ReviewBox labels the file as having no textual patch and explains that
it may be binary, rename/mode-only, or empty: “No GitHub text patch (binary,
rename/mode-only, or empty file).” When GitHub reports changes but omits the
patch, ReviewBox labels it as “binary or too large.” Neither state invents
content, and a present-but-empty patch remains a distinct “No textual changes”
state.
Locally retained details are limited to the first 300 files, 256 KiB or 5,000
patch lines per file, 2,000 characters per logical diff line, and 2 MiB of patch
text per commit. The in-process detail cache retains at most 16 commits. A file
or commit-budget cap, additional file page, or response over the 16 MiB command
limit is shown explicitly. Available diff text remains scrollable and long lines
soft-wrap instead of being horizontally discarded.

Authentication, missing `gh`, offline/transport, permission/not-found,
rate-limit, malformed-response, malformed-JSON, and other API/command failures
are separate sanitized states. These displays never include stderr, response
bodies, repository names, owners, branches, commit SHAs, or subjects. Partial
repository/branch failures retain usable results and mark coverage incomplete;
fatal discovery failures and genuinely empty successful loads remain distinct.

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

Before an external editor starts, ReviewBox shows the cursor, leaves the
alternate screen, and disables raw mode. After the editor returns—even after a
nonzero exit or launch/read failure—it enables raw mode, enters the alternate
screen, hides the cursor, and fully redraws. If terminal reacquisition fails,
already-read content is still offered to the draft store before ReviewBox exits
with a sanitized error; final teardown remains idempotent.

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
