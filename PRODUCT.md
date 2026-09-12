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

- Today is the default date; allow choosing another day and a timezone (default
  to the local timezone). Explain precisely which commit timestamp is filtered.
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

## Delivery order

1. Runnable terminal foundation and demo data with Neovim-style navigation.
2. GitHub repository discovery and date-filtered commit loading.
3. File/diff review and durable reviewed state.
4. Comment drafts, external editor, and explicit GitHub publishing.
5. End-to-end polish, checks, and installation documentation.

Each queue task must leave a runnable increment. Forge owns the implementation,
review gates, focused local commits and push after a successful queue task.
