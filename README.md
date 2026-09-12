# ReviewBox

A keyboard-first terminal review inbox for commits across GitHub projects.

See [PRODUCT.md](PRODUCT.md) for the accepted requirements and delivery order.

## Current status: terminal foundation

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

Planned for later delivery increments, and not implemented yet:

- Live GitHub access and repository/commit discovery.
- Durable review state and reviewed/unreviewed filtering.
- Real diffs loaded from GitHub repositories.
- Drafting, editing, persistence, and publishing of comments.
- Date/timezone selection, authentication integration, and repeated-match
  navigation with `n` / `N`.

## Prerequisites and demo

Install Rust 1.88 or newer (including Cargo) and use a terminal that supports
Crossterm. Launch the interactive demo with:

```sh
cargo run -- --demo
```

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

## Terminal restoration

On a normal quit or a propagated application error, ReviewBox restores the
cursor, leaves the alternate screen, and disables raw mode in reverse setup
order. Setup tracks acquired resources so a recoverable partial setup failure
also restores what was already changed. Process aborts, `SIGKILL`, power loss,
and equivalent failures cannot be guaranteed to run cleanup.

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
