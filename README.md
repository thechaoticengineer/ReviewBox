# ReviewBox

A keyboard-first terminal inbox for reviewing your own GitHub commits across
projects.

**Current status: delivery increment 5 of 5 — usability and release readiness.**
The first usable release described in [PRODUCT.md](PRODUCT.md) is implemented:
live and fictional inboxes share the repository → commit → file → diff workflow,
reviewed progress and comment drafts are durable in live mode, comments require
an explicit publish confirmation, loading and failure states are actionable, and
the terminal UI has compact layouts and deterministic end-to-end smoke coverage.

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

With no options, live mode chooses today in the detected local IANA timezone.
Choose a date, a timezone, or both:

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
combined with `--demo`, `--demo-smoke`, or `--help`.

Run the interactive fictional demo or its noninteractive smoke workflow with:

```sh
cargo run -- --demo
cargo run -- --demo-smoke
```

The demo uses fictional fixtures, makes no GitHub request, and keeps review and
draft state in memory. The smoke drives the complete workflow through an
in-memory terminal, fake comment service, mock editor, memory-only stores, and
recorded terminal operations. It never invokes `gh`, opens a real editor,
reaches the network, or writes to HOME/XDG storage.

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
ReviewBox processes are not locked and can race.

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
| `h / l` | focus previous / next pane |
| `j / k` | move or scroll down / up |
| `gg / G` | first / last position |
| `Ctrl-d / Ctrl-u` | move down / up half a pane |
| `Enter / Escape` | open child / return to parent |
| `/` | search the focused pane |
| `n / N` | next / previous search match |
| `m` | mark commit reviewed / unreviewed |
| `f` | show remaining / all commits |
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
| `Help: j/k or arrows` | scroll this binding list |
| `Help: Escape` | close help; other keys stay isolated |

Edit mode:

| Keys | Action |
| --- | --- |
| `Edit: printable / Enter` | insert text / newline |
| `Edit: arrows / Home / End` | move the text cursor |
| `Edit: Esc / Ctrl-g` | save and return / cancel |
| `Edit: Ctrl-e` | edit current buffer externally |
| `Edit: Ctrl-c` | save and quit |

Comments mode:

| Keys | Action |
| --- | --- |
| `Comments: j/k or arrows` | scroll comments |
| `Comments: r / Esc` | refresh / close |

Publish confirmation:

| Keys | Action |
| --- | --- |
| `Publish: y / any other key` | confirm / cancel |

The in-app `?` overlay is generated from the same binding descriptions. Search,
Help, Edit, Comments, and Publish modes isolate their keys from Normal mode.
Normal navigation letters are inserted literally in Edit mode.

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
They cover day/timezone presentation, all four panes, navigation and search,
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
commit, not universal coverage. Live permission-denied, authentication-expired,
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
- Repository, branch, and commit pages request 100 items and continue until a
  short or empty page; there is no smaller local page cap. Commit-comment lists
  are capped at 10 pages of 100 and are labeled incomplete when the cap or an
  oversized page is encountered.
- GitHub receives the branch, authenticated `author`, and UTC `since`/`until`
  interval. ReviewBox defensively keeps only a case-insensitive exact match on
  top-level `author.login` and filters `commit.author.date` into `[start, end)`.
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
