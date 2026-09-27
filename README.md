# ReviewBox

A keyboard-first terminal inbox for reviewing your own GitHub commits across
projects.

**Current status: backlog release, following the completed first release.**
The first usable release described in [PRODUCT.md](PRODUCT.md) is implemented:
live and fictional inboxes share the repository → commit → file → diff
workflow, reviewed progress and comment drafts are durable in live mode,
comments require an explicit publish confirmation, loading and failure states
are actionable, and the terminal UI has compact layouts and deterministic
end-to-end smoke coverage. On top of that first release, this backlog release
adds a durable, undated review backlog with progressive history loading and
reversible bulk review marking.

The default inbox is now an undated **backlog** of your unreviewed commits: a
commit stays pending until you mark it reviewed, however old it is. History
loads progressively with in-app Load older / Load all, progress, cancellation,
and explicit completeness states. You can select single commits or all loaded
commits of the current repository and mark them reviewed, or unreviewed from the
Reviewed view, in one confirmed action (see
[Selecting commits and bulk review](#selecting-commits-and-bulk-review)).

## Prerequisites

- Rust 1.88 or newer, including Cargo. This matches `package.rust-version` in
  `Cargo.toml`.
- A terminal supported by Crossterm. A 60×16 terminal is the smallest full
  four-pane layout; smaller terminals use a compact, panic-free status view.
- For live mode only: the `gh` executable, network access, and an authenticated
  GitHub CLI session. Demo modes need none of these.

## Install, build, and launch

Install the local checkout with Cargo and launch the installed executable:

```sh
cargo install --path . --locked
reviewbox --help
reviewbox
```

The release build can instead be run directly from the checkout:

```sh
cargo build --release
./target/release/reviewbox --help
./target/release/reviewbox
```

### Default: the review backlog

With no options, ReviewBox opens the backlog: your unreviewed commits across the
repositories you own, with **no date limit**. A commit stays in the backlog until
you explicitly mark it reviewed with `m`; opening or browsing a commit never
marks it, and a long absence does not expire anything. Marks persist across
restarts (see [Storage and privacy](#storage-and-privacy)).

Loading is progressive so the terminal stays responsive:

1. The initial batch discovers your owned repositories, enumerates their
   branches, and fetches the **first page (up to 100 commits) of every branch**.
   This is only a fetch optimization, never an age cutoff.
2. `o` (**Load older**) fetches one more page of every branch of the selected
   repository. `O` (**Load all**) keeps paging that repository until its history
   is complete, a failure makes it incomplete, or you cancel.
3. `x` cancels the active load promptly. Commits loaded before cancellation stay
   browsable, and the status reads `Cancelled — N commits loaded, history
   incomplete`.

Only one history load runs at a time. A repository whose loaded commits are all
reviewed stays listed while older history may exist, so old pending commits
remain reachable. Repository rows show the pending count and a coverage marker:
`+older` (older history not loaded), `!incomplete` (a request failed), or
`loading` (initial batch pending); complete repositories have no marker. The
diff pane shows a backlog summary with progress, per-repository coverage, and a
bounded, sanitized error list whenever no commit detail is open.

Completeness is stated explicitly and never overstated:

| State | Meaning |
| --- | --- |
| `Loading backlog: …` / `Loading older history: N pages, M commits loaded` | A load is running; `x` cancels. |
| `Backlog clear — all history loaded and reviewed` | Discovery succeeded, every repository's history is complete, and no loaded commit is pending. Only this state claims an empty backlog. |
| `No pending commits in loaded history — older history not loaded (o/O)` | Everything loaded is reviewed, but older pages may still hold pending commits. |
| `History incomplete: <reason>` | A branch or page failed, for example `GitHub API rate limit reached`; press `o`/`O` to retry. |
| `Cancelled — N commits loaded, history incomplete` | You cancelled a load. |
| `NO OWNED REPOSITORIES` / `LOAD FAILED — <reason>` | Discovery finished empty or failed. |

`f` switches between the backlog of unreviewed commits and a **Reviewed** view
that lists loaded reviewed commits so you can inspect them and press `m` to
unmark one; it returns to the backlog. There is no date filter.

### Selecting commits and bulk review

Selection and the reviewed flag are separate. Selecting a commit only picks it
for the next bulk action; it never marks anything, and it is never saved.
Selection is available in the backlog and Reviewed views (not in the single-day
view) with the commit pane or a deeper pane focused:

- `Space` toggles the current commit. Selected rows show `*` before the review
  marker, and the commit pane title shows `N selected (loaded)`.
- `A` selects every **loaded** commit shown in the current repository and view,
  or clears the selection when all of them are already selected.
- `m` with a selection opens a confirmation instead of toggling one commit;
  without a selection it keeps its single-commit behavior. In the backlog the
  action marks the selection reviewed; in the Reviewed view it marks the
  selection unreviewed, returning those commits to the backlog.

The confirmation freezes the exact commits when it opens and states the action,
the count, the repository, and the scope:

| Coverage of the repository | Scope wording |
| --- | --- |
| Complete, every shown commit selected | `all N unreviewed commits in <repo>` (Reviewed view: `all N reviewed commits`) and `complete history loaded` |
| Complete, some selected | `N selected commits in <repo>; complete history loaded` |
| Older history not loaded | `N selected loaded commits …; older history is not loaded and is not included` |
| History incomplete after a failure | `N selected loaded commits …; history is incomplete (some history is unavailable) and is not included` |

"All" always refers to the commits of the current filtered view of one
repository, never to every commit of the repository. Press `O` first to make
the full supported history selectable. `y` saves exactly the frozen commits in
one atomic write of `review-state.json`; any other key cancels and keeps the
selection. Commits loaded after the confirmation opened are never included and
stay unreviewed. If saving fails, the status reports the failure, and marks,
drafts, and the selection stay unchanged. There is no cross-repository bulk
action and no date watermark.

Selection lifecycle: it belongs to one repository and one view. It is cleared
when you move to another repository, when `f` switches the view, and after a
successful bulk action. Background loading never adds commits to it; it follows
commit identity (repository and full SHA) through reordering and drops any
commit that is no longer shown. Moving focus between panes keeps it.

### Single-day view: `--date` / `--timezone`

The previous day-scoped inbox remains available when you pass `--date`,
`--timezone`, or both. It is labeled as a single-day view and does not affect
the default backlog. A missing date means today; a missing timezone means the
detected local IANA timezone:

```sh
reviewbox --date 2026-09-12
reviewbox --timezone Europe/Warsaw
reviewbox --date 2026-03-29 --timezone Europe/Warsaw
```

The selected day is the half-open interval from local midnight through, but not
including, the following local midnight. Each boundary is converted to UTC
independently, so daylight-saving days may be 23 or 25 hours. Invalid arguments
exit with status 2 before terminal setup. If local timezone detection fails,
ReviewBox visibly falls back to `Etc/UTC`. `--date` and `--timezone` cannot be
combined with `--demo`, `--demo-smoke`, or `--help`. In the single-day view,
`f` keeps its earlier meaning (all commits / remaining commits only), and `o`,
`O`, and `x` are unavailable.

Run the interactive fictional demo or its noninteractive smoke workflow with:

```sh
cargo run -- --demo
cargo run -- --demo-smoke
```

The demo is a fictional backlog served by an in-memory history pager through the
same background loader as live mode. It makes no GitHub request and keeps
review and draft state in memory. The last repository,
`fictional-harbor/tidepool-archive-demo`, has a first page that is already
marked reviewed, so it starts with `(0) +older`; `o` reveals a pending commit
from 2023 and `O` loads the rest. Older demo pages arrive with a short delay so
progress and `x` are observable.

The smoke drives the static fixture workflow and the backlog demo through an
in-memory terminal, fake comment service, mock editor, memory-only stores, and
recorded terminal operations. For the backlog it loads older history, marks the
old pending commit reviewed, confirms that it leaves the backlog and appears in
the Reviewed view, unmarks it, opens and cancels a partial-scope bulk
confirmation, loads all history, selects all loaded commits, bulk marks them,
and bulk unmarks them from the Reviewed view. It never invokes `gh`,
opens a real editor, reaches the network, or writes to HOME/XDG storage.

## GitHub authentication and access

ReviewBox reuses the active GitHub CLI session. It does not ask for, print, or
persist a token. Authenticate and check the active session with:

```sh
gh auth login
gh auth status
```

Prefer a fine-grained token limited to the repositories you want ReviewBox to
see. GitHub's current endpoint documentation requires repository **Metadata:
read** for comment listing and **Contents: read** for repository contents,
commits, and creating a commit comment. Organization policy can further restrict
commit comments. For a manually supplied token, the GitHub CLI manual recommends
`GH_TOKEN` for fine-grained tokens; its `gh auth login --with-token` flow requires
the broader classic-token scopes `repo`, `read:org`, and `gist`. Never put a token
in this repository or a command-line argument. Grant only what your workflow
needs and review the current [GitHub commit-comment permissions](https://docs.github.com/en/rest/commits/comments)
and [`gh auth login` guidance](https://cli.github.com/manual/gh_auth_login).

Browsing uses explicit `gh api --method GET` requests. `C` also performs only a
GET. The sole write path is
`POST /repos/{owner}/{repo}/commits/{full_sha}/comments`, and it starts only
after `P` on a saved draft followed by `y` in the confirmation overlay. To keep
a session strictly read-only, do not complete `P`/`y`.

## Storage and privacy

Live mode resolves both durable files from the same user-data directory:

| Data | Absolute `XDG_DATA_HOME` | Fallback when XDG is unset, empty, or relative |
| --- | --- | --- |
| Reviewed progress | `$XDG_DATA_HOME/reviewbox/review-state.json` | `$HOME/.local/share/reviewbox/review-state.json` |
| Comment drafts | `$XDG_DATA_HOME/reviewbox/comment-drafts.json` | `$HOME/.local/share/reviewbox/comment-drafts.json` |

The HOME fallback is used only when `HOME` is absolute. If neither base is
usable, ReviewBox continues browsing with a warning and disables the affected
file-backed store; it never writes relative to the working directory. On Unix,
new ReviewBox data directories are created with mode `0700` and replacement
files with mode `0600`. Existing parent-directory permissions are not repaired.
Writes use a same-directory temporary file and atomic rename, but concurrent
ReviewBox processes are not locked and can race. A review change covering several
commits is saved as a single atomic replacement of `review-state.json`; if that
save fails, the previously saved marks remain unchanged.

`review-state.json` contains only a version, numeric GitHub repository IDs, and
complete lowercase commit SHAs. `comment-drafts.json` contains repository IDs,
complete SHAs, line paths/positions, submission markers, and **comment bodies in
plaintext**. Treat the draft file and backups of it as sensitive personal data.
It contains no GitHub token. Invalid, unreadable, unsupported, or oversized
state is not silently overwritten.

External editing uses the first nonblank `$VISUAL`, then `$EDITOR`, without a
shell. A private `reviewbox-editor-*` directory and `draft.md` are created under
an absolute `$XDG_RUNTIME_DIR`, or otherwise the operating system temporary
directory. On Unix their modes are `0700` and `0600`. The body is never placed
on the editor command line; temporary cleanup is best-effort after every editor
path. Abrupt process or machine termination can leave plaintext temporary data,
so inspect the applicable runtime/temp directory after such a failure.

Generated binaries (`target/`), `.forge/`, `.reviewbox/`, `.env*`, logs, and
provider runtime directories are ignored. Do not add tokens, captures, draft
bodies, or personal repository data to Git.

## Keybindings

Normal mode:

| Keys | Action |
| --- | --- |
| `h / l or Left / Right` | focus previous / next pane |
| `j / k or Down / Up` | move down / up; diff: one display row |
| `gg / G` | first / last position |
| `Home / End` | first / last position |
| `Ctrl-d / Ctrl-u` | move down / up half a pane |
| `PageDown / PageUp` | move down / up a full pane |
| `[ / ]` | diff: previous / next hunk |
| `Enter / Escape` | open child / return to parent |
| `/` | search the focused pane |
| `n / N` | next / previous search match |
| `m` | mark reviewed / unreviewed; bulk if selected |
| `Space / A` | select commit / all loaded (backlog) |
| `f` | backlog / reviewed (day: remaining / all) |
| `o / O / x` | load older / all history; cancel loading |
| `c` | edit commit or selected-line draft |
| `E` | edit draft with `$VISUAL / $EDITOR` |
| `P` | publish draft after confirmation |
| `C` | open existing commit comments |
| `?` | open this help |
| `q` | quit from normal mode |
| `Ctrl-c` | quit globally; Edit must save first |

Search mode:

| Keys | Action |
| --- | --- |
| `Search: text, Backspace` | edit the query |
| `Search: Enter / Escape` | apply / cancel |

Help mode:

| Keys | Action |
| --- | --- |
| `Help: j/k, arrows / Esc` | scroll / close; other keys stay isolated |

Edit mode:

| Keys | Action |
| --- | --- |
| `Edit: printable / Enter` | insert text / newline |
| `Edit: arrows / Home / End` | move the text cursor |
| `Edit: Esc / Ctrl-g` | save and return / cancel |
| `Edit: Ctrl-e / Ctrl-c` | edit buffer externally / save and quit |

Comments mode:

| Keys | Action |
| --- | --- |
| `Comments: j/k or arrows` | scroll comments |
| `Comments: r / Esc` | refresh / close |

Publish and bulk review confirmations:

| Keys | Action |
| --- | --- |
| `Publish, Bulk: y / other` | confirm / cancel (cancel keeps selection) |

In Normal mode, arrow keys (`Left`, `Right`, `Up`, `Down`) perform the same actions as `h`, `l`, `k`, `j` respectively across the repository, commit, file, and diff panes. The in-app `?` overlay is generated from the same binding descriptions. Search,
Help, Edit, Comments, Publish, and Bulk confirmation modes isolate their keys
from Normal mode; `Space`, `A`, and `m` cannot start a bulk change from them.
Normal navigation letters are inserted literally in Edit mode.

### Reading diffs

The diff pane wraps long patch lines to the pane width and moves by
**display row**: `j`/`k` and `Up`/`Down` step through every wrapped row,
including the middle and end of a single line taller than the pane.
`Ctrl-d`/`Ctrl-u` move half a pane and `PageDown`/`PageUp` a full pane; the
viewport and cursor move together, so consecutive pages show adjacent rows
without skipping any, and the last page stops at the patch end without blank
overrun. `Home`/`End` (like `gg`/`G`) jump to the start or end of the patch;
in the other panes they select the first or last item, and in Edit mode they
still move the text cursor. `]` and `[` jump to the next or previous hunk
header, placing it at the top of the pane when possible, and report
`Already at last hunk` / `Already at first hunk` at the boundaries. The page
size excludes the pane borders and any wrapped notices, such as a local patch
cap.

The Diff title shows `patch row R/T • hunk H/N`. It counts rows of the patch
GitHub returned, not of the full file; ReviewBox never fetches or shows file
content beyond the patch. Only the first wrapped row of a patch line shows the
draft/comment marker and old/new line numbers. A comment on any wrapped row
targets the same original patch line and GitHub position; hunk headers,
no-newline markers and unsupported rows cannot take a line comment. Search
(`/`, `n`, `N`) matches patch lines case-insensitively and moves to the wrapped
row that contains the match. Resizing keeps the current line and position
within it, and a cursor at the patch end keeps following the end.

Planned, not yet available: a labeled side-by-side old/new view with a unified
fallback, choosing the old or new side for line actions, and an expanded
full-terminal diff view.

## Terminal restoration

ReviewBox acquires raw mode, the alternate screen, and a hidden cursor in that
order. Normal `q`, global `Ctrl-c`, propagated application errors, loading
cancellation, and recoverable partial setup failures restore the cursor, leave
the alternate screen, and disable raw mode in reverse order. Restoration is
idempotent, with `Drop` as a fallback.

Before an external editor starts, ReviewBox restores the normal terminal. It
reacquires raw mode, alternate screen, cursor hiding, terminal size, and a full
redraw afterward, including editor failure paths. `SIGKILL`, process abort,
power loss, and a terminal/editor that disrupts the process group cannot be
guaranteed to run cleanup. If a shell is left in an unusual state, try `reset`
or `stty sane`, then remove any leftover private editor temporary directory.

## Verification coverage and limitations

### Fixture-tested behavior

The test suite and `--demo-smoke` use fictional, network-incapable dependencies.
Arrow-key parity (`Left`/`Right`/`Up`/`Down` mirroring `h`/`l`/`k`/`j`) is
covered across the repository, commit, file, and diff panes, with edit mode
still moving the text cursor and Help/Comments/Search/Publish/Bulk modes
either scrolling consistently or staying isolated; a dedicated test asserts
every key dispatched in Normal mode, including the arrows, `o`/`O`/`x`,
`Space`, and `A`, has a matching `?` help entry.
Diff navigation tests cover display-row, half-page, full-page, start/end and
hunk movement at exact viewport boundaries (with and without wrapped notices),
every wrapped row of a line taller than the pane, exact-fit tails, resize
anchor preservation, search into a wrapped continuation, comment targets on
continuations resolving to the original GitHub position, refusal on hunk
headers, no-newline markers and unsupported rows, isolation of the new keys
in every modal mode, and that cursor movement reuses the cached display
projection while width changes and same-file content replacement rebuild it.
Backlog tests drive the real background loader with scripted `gh` responses and
in-memory pagers. They cover full raw pages whose accepted projection is empty
followed by older pages (an empty initial batch), entirely reviewed first
batches, multiple pages of incremental history, overlapping branches,
selection kept on the same SHA during incremental loading, cancellation during
Load all, rate-limited branches left incomplete and then retried (partial
failure and rate limits), discovery failure, shutdown of blocked loads,
stale-generation events, restart persistence with a temporary
`review-state.json`, existing marks and drafts loading unchanged, and that
viewing a commit never marks it. Bulk review tests cover single, several, and
all-loaded selection, confirmed bulk mark and Reviewed view bulk unmark,
partial versus complete scope wording before and after Load all,
incomplete-history disclosure, a page arriving between confirmation and `y`
staying excluded and pending (concurrently discovered commits are never
included), selection following commit identity through reordering and
pruning, clearing on repository and view changes, a failing store preserving
prior marks, drafts, and selection on storage failure, restart persistence of
bulk marks, and modal isolation. The demo smoke confirms a partial-scope bulk
request and cancels it, then selects all after Load all, bulk marks, opens the
Reviewed view, and bulk unmarks. They also
cover day/timezone presentation, all four panes, navigation and search,
representative and truncated diffs, resize/compact rendering, reviewed/filter
state, commit and eligible-line drafts, unsupported-line refusal, external
editing with terminal suspend/resume, existing comments, confirmed mocked
publish success, mocked `422` retention, and clean `q`/`Ctrl-c` event-loop exits.
They do not prove GitHub permissions or publish a real comment.

### Read-only live verification

Increment 5 release qualification found an available authenticated `gh` session
and network. With all output kept private, it successfully checked
`gh auth status`, `GET /user`, and paginated owner-repository GET data; ReviewBox
then reached a complete inbox, opened a matching commit, loaded its changed-file
detail, browsed a diff, loaded existing commit comments with `C`, closed the
overlay, and exited with `q`. The qualifying TUI pass used only
`j`, `Enter`, `C`, `Escape`, and `q`. It used no `m`, `c`, `E`, `P`, or `y`, sent
no POST/PATCH/PUT/DELETE request, and published no comment. No account name,
repository name, SHA, path, body, token, or capture is retained in the project.

This is evidence for one available authenticated environment and suitable
commit, not universal coverage. It predates the backlog: the default undated
backlog, Load older, Load all, cancellation, and bulk selection/marking are
fixture-tested only and have not been verified against GitHub. Live permission-denied, authentication-expired,
offline, rate-limited, oversized-response, empty-account, empty-day, and
incomplete-branch outcomes remain fixture-tested rather than induced against
GitHub. Live line-position publishing was deliberately not tested.

### GitHub and local limits

- Discovery calls `GET /user`, then paginates `GET /user/repos` with
  `affiliation=owner`. It retains repositories whose returned owner login exactly
  matches the authenticated login, case-insensitively. Organization-owned and
  collaborator-only repositories are outside the inbox, even when accessible.
- Every currently returned branch is paginated and queried independently.
  Tag-only, dangling, deleted-ref, or inaccessible-ref commits are not covered;
  refs can also change during traversal, so this is not a transactional snapshot.
  Backlog branches are paged independently by page number over a possibly long
  session; if a branch changes between requests, a commit can be missed or seen
  twice. Duplicates are removed by SHA, but ReviewBox does not claim a stable
  snapshot of GitHub history.
- Repository, branch, and commit pages request 100 items. The single-day view
  continues until a short or empty page. The backlog fetches one commit page per
  branch initially and more only on `o`/`O`; a branch is exhausted only by a
  short or empty raw page, regardless of how many entries pass the author
  filter. Load all on a large history can take many requests and may reach the
  GitHub API rate limit; the repository is then shown as incomplete and keeps
  what was loaded. Commit-comment lists
  are capped at 10 pages of 100 and are labeled incomplete when the cap or an
  oversized page is encountered.
- The backlog sends GitHub the branch and authenticated `author` with no date
  bounds. The single-day view also sends the UTC `since`/`until` interval.
  ReviewBox defensively keeps only a case-insensitive exact match on top-level
  `author.login`; the single-day view also filters `commit.author.date` into
  `[start, end)`.
  It does not use committer time, push time, contribution-calendar time, or
  author name/email. A null top-level GitHub author is excluded.
- A full SHA reached from multiple branches is deduplicated within its repository,
  not across repositories. Repository and branch changes during loading may still
  make coverage incomplete.
- A single `gh` response is capped at 16 MiB. Commit detail retains the first 300
  files, 256 KiB or 5,000 patch rows per file, 2,000 characters per logical row,
  and 2 MiB of patch text per commit. The detail cache retains 16 commits. Every
  local or response cap is shown as incomplete/truncated; omitted GitHub patches
  remain explicitly unavailable, binary/too-large, or empty rather than invented.
- Authentication, missing `gh`, transport/offline, permission/not-found,
  rate-limit, malformed response/JSON, truncation, and other failures are
  sanitized and distinct. A 403 is classified as rate-limited only when headers
  report zero remaining requests or a retry interval; repository/branch failures
  preserve partial data and mark it incomplete, while discovery failure is fatal.
- Line comments are available only for retained context, addition, or deletion
  rows in a textual patch after a valid `@@` hunk header. File/hunk headers,
  no-newline notices, malformed rows, binary/unavailable patches, and rows omitted
  by a local cap are unsupported. GitHub positions are derived from raw patch rows
  starting at the first hunk, including intervening headers and notices. This
  mapping is fixture-tested only; a GitHub rejection preserves the draft. These
  are commit comments, not pull-request review threads.

## Release checks

Run the complete local gate from the repository root:

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo run -- --demo-smoke
cargo build --release
./target/release/reviewbox --help
./target/release/reviewbox --demo-smoke
```

The release artifact is `target/release/reviewbox`; `target/` is intentionally
ignored. The local-path install was separately qualified with
`cargo install --path . --locked` into an isolated temporary Cargo root, followed
by the installed executable's `--help` command.

## License

[MIT](LICENSE) © 2026 thechaoticengineer.
