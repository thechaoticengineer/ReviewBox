# ReviewBox

A terminal review inbox for commits across your GitHub projects, with
Neovim-style navigation and comment editing.

See [PRODUCT.md](PRODUCT.md) for the accepted requirements and delivery order.

## Status

The demo now provides populated repository, commit, file, and diff panes backed
by deterministic fictional fixtures. Normal-mode navigation keeps parent and
child selections coherent, clamps movement at every boundary, redraws after a
terminal resize, and uses a compact fallback on small terminals. Terminal state
is restored after normal exit and propagated application errors.

Search, keyboard help, live GitHub access, review persistence, real GitHub
diffs, and comments remain planned work.

## Run the demo

Install Rust 1.88 or newer, then run:

```sh
cargo run -- --demo
```

The demo performs no network requests, needs no credentials, and writes no
runtime state.

Implemented normal-mode bindings:

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
- `q` / `Ctrl-c`: quit and restore the terminal.

The status line shows normal mode, a pending `g`, the focused pane, and the most
recent action. `/` search and `?` help are planned for the next stage.

Development checks for this stage are:

```sh
cargo fmt --check
cargo build
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

## Goal

Choose a day and quickly review your commits across all your GitHub repositories
in one place, without opening each project separately.

## Planned first version

- Connect to your GitHub account.
- Select a date, with today as the default.
- Collect your commits across your repositories and group them by project.
- Show commit messages, timestamps, and links to the commits on GitHub.
- Open changes for review and keep track of reviewed commits locally.
- Support private repositories when the connected account has permission.

## Decisions for implementation

- Refine the terminal pane layout and modal keybindings.
- Define the date filter and timezone behavior.
- Define branch coverage and handling of forked or archived repositories.
- Choose authentication and local storage for review progress.

## Related project

[CommitPulse](https://github.com/thechaoticengineer/CommitPulse) is a planned
Omarchy widget for daily, weekly, monthly, and yearly GitHub contribution counts.

## License

[MIT](LICENSE) © 2026 thechaoticengineer.
