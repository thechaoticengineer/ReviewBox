# ReviewBox

A terminal review inbox for commits across your GitHub projects, with
Neovim-style navigation and comment editing.

See [PRODUCT.md](PRODUCT.md) for the accepted requirements and delivery order.

## Status

Project initialized. This repository currently contains the project brief and
license; the application is not implemented yet. The accepted direction is a Rust
terminal UI with modal keyboard navigation, diff review and comment drafts.

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
