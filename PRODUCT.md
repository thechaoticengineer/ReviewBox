# ReviewBox product brief

## Agreed direction

Build a standalone terminal application for daily review of the user's own GitHub
commits across their projects. The interaction should feel familiar to a Neovim
user: keyboard first, modal navigation, focused panes, and fast movement through
repositories, commits, files, and diffs. Comment writing is a core feature.

Use Rust and a maintained terminal UI library. Keep the application small and
usable on Linux/Omarchy. Reuse the installed GitHub CLI authentication without
storing tokens. Persist review state and draft comments in an XDG user data
directory, outside Git repositories.

## First usable version

- Allow choosing a single day and a timezone (default to the local timezone) as
  an explicit view. Explain precisely which commit timestamp is filtered. The
  original "today is the default date" requirement is superseded by the
  backlog direction below.
- Discover all repositories owned by the authenticated user, including private
  ones where authorized, with pagination. Filter commits by the user's authorship.
  Include branch coverage, deduplicate shared SHAs within each repository, and
  disclose unavailable repositories, API limits, and incomplete results.
- Repository/commit/file panes and a readable scrollable diff, with loading,
  empty, offline, permission-error and truncated-diff states.
- Normal mode: h/j/k/l, gg/G, Ctrl-d/Ctrl-u, / search, n/N, Enter to open,
  Escape to return, and ? for discoverable keyboard help. Editing mode must
  accept text without triggering navigation commands. Support terminal resizing.
- Write and edit comments on commits and supported diff lines. Save drafts
  across restarts, and support editing through $VISUAL/$EDITOR (including nvim).
- Explicitly publish a user-written comment to GitHub and display success or
  failure. Keep failed drafts. Prevent accidental duplicate submissions. Use the
  appropriate commit comment API; do not pretend a commit is a pull request.
- Mark commits reviewed/unreviewed, persist progress by repository and full SHA,
  and filter the remaining review inbox.
- Provide a demo/fixture mode so the UI can be exercised without credentials or
  network writes. Tests must not post comments to real repositories.
- Document installation, launch, keybindings, authentication, data storage,
  coverage limitations, and verification commands.

## Accepted backlog direction

This direction supersedes the original default-today requirement.

Delivered in the backlog and bulk-review increments:

- The default inbox is the user's unreviewed commits across the supported
  repositories, with no date limit. A commit stays pending until it is
  explicitly marked reviewed, regardless of age or a long absence; opening a
  commit does not mark it. Marks survive restarts; existing marks and drafts are
  preserved.
- History loads progressively. The initial batch is only a fetch optimization.
  In-app Load older (`o`) and Load all (`O`) work on the current repository with
  progress, cancellation (`x`), and accurate complete, incomplete, rate-limited,
  and cancelled states. Old commits stay reachable even when the first batch is
  empty or entirely reviewed. An incomplete load is never shown as an empty or
  fully reviewed backlog.
- `--date` and `--timezone` remain as an explicitly labeled single-day view; they
  do not limit the default backlog. There is no date-range picker or date filter.
- A Reviewed view (`f`) exists only to inspect loaded reviewed commits and
  unmark them, returning them to the backlog.
- In normal browsing mode, arrow keys mirror `h`/`j`/`k`/`l`.
- Selecting individual commits (`Space`) or all loaded commits (`A`) in the
  current repository and view, and marking the selection reviewed (backlog) or
  unreviewed (Reviewed view) in one confirmed action (`m`, then `y`) with an
  explicit count, repository, and scope. Selection stays distinct from the
  durable reviewed flag, is cleared on repository or view changes and after a
  successful action, never covers unloaded history silently, and never applies
  to commits discovered after the confirmation opened.

### Out of scope

These were considered while designing the backlog increment and are
deliberately not supported. They are not promised future work; revisiting
either requires a new agreed direction, not an implicit extension of the above.

- A global, cross-repository bulk operation. Bulk marking always acts within
  one repository and one view (backlog or Reviewed), never across
  repositories.
- Any date-range picker or other new date-filter UI. `--date` and
  `--timezone` remain exactly the existing explicitly labeled single-day view
  and do not gain new UI; they do not limit or otherwise control the default
  backlog.

## Accepted diff presentation direction

Improve diff reading without changing review, backlog, comment or storage
behavior: navigation by wrapped display row with page, start/end and hunk
movement; a labeled side-by-side old/new view with synchronized scrolling and
an automatic unified fallback in narrow panes; a distinct control for
choosing the old or new side for line actions; and an in-app expanded diff
view that restores the browsing context. Line comments always resolve to the
canonical GitHub patch position, and no content beyond the patch is fetched.

Delivered so far: display-row navigation (`j`/`k`, `Ctrl-d`/`Ctrl-u`,
`PageDown`/`PageUp`, `Home`/`End`, `gg`/`G`, `[`/`]` hunks), patch-row and hunk
position feedback, wrapped-row comment targeting and search, and resize anchor
preservation. The side-by-side view, side selection and expanded view are
still in progress.

## Delivery order

1. Runnable terminal foundation and demo data with Neovim-style navigation.
2. GitHub repository discovery and date-filtered commit loading.
3. File/diff review and durable reviewed state.
4. Comment drafts, external editor, and explicit GitHub publishing.
5. End-to-end polish, checks, and installation documentation.

Each queue task must leave a runnable increment. Forge owns the implementation,
review gates, focused local commits and push after a successful queue task.
