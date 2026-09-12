# ReviewBox

A terminal review inbox for commits across your GitHub projects, with
Neovim-style navigation and comment editing.

See [PRODUCT.md](PRODUCT.md) for the accepted requirements and delivery order.

## Status

The first terminal-foundation stage is implemented. `--demo` opens an offline,
fictional repository/commit/file/diff pane shell, redraws after terminal resize,
uses a compact fallback on small terminals, and restores terminal state on exit.

Pane navigation, search, keyboard help, live GitHub access, review persistence,
real diffs, and comments remain planned work.

## Run the demo

Install Rust 1.88 or newer, then run:

```sh
cargo run -- --demo
```

The demo performs no network requests, needs no credentials, and writes no
runtime state. Press `q` (or `Ctrl-c`) to quit. `Escape` does not quit; it is
reserved for returning or closing transient modes in later stages.

Development checks for this stage are:

```sh
cargo fmt --check
cargo build
cargo test
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
