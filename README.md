<h1 align="center">tmux-atelier</h1>

<p align="center"><strong>One workbench for every project, wherever it lives.</strong></p>

<p align="center">
  <a href="https://github.com/inayayousfi/tmux-atelier/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/inayayousfi/tmux-atelier/actions/workflows/ci.yml/badge.svg"></a>
  <a href="https://github.com/inayayousfi/tmux-atelier/releases"><img alt="Release" src="https://img.shields.io/github/v/release/inayayousfi/tmux-atelier"></a>
  <a href="https://github.com/inayayousfi/tmux-atelier/stargazers"><img alt="GitHub stars" src="https://img.shields.io/github/stars/inayayousfi/tmux-atelier"></a>
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/github/license/inayayousfi/tmux-atelier"></a>
</p>

tmux-atelier keeps local and remote project workspaces together in the same tmux client.

## Workspaces

A workspace starts with a directory on a machine. It can be local or reached over SSH. Open one, then give each part of the project its own tab: Neovim, an agent CLI, a plain shell, or whatever else you need.

New tabs and splits start on that machine in the project directory, ready for the next command.

One shortcut takes you to another workspace, even when the next project lives on a different machine. You do not have to detach, hunt down another session, reconnect, and find your way back to its directory.

The workspace interface feels modern. Underneath it, tmux keeps the sessions and processes alive, while OpenSSH handles remote connections.

## Quick start

Install it with:

```sh
curl -fsSL https://raw.githubusercontent.com/inayayousfi/tmux-atelier/main/install.sh | bash
```

The installer prints the line to add at the end of your tmux configuration. With the default installation path, it looks like this:

```tmux
run-shell ~/.config/tmux/tmux-atelier/tmux-atelier.tmux
```

Reload tmux and press your prefix followed by `N` to make your first workspace. If you keep plugins somewhere else, the installer accepts `--install-dir`.

The installer supports Linux x86-64, Linux ARM64, and macOS Apple Silicon. Remote workspaces use the OpenSSH configuration and credentials you already have.

## Guide

Configuration, commands, restoration behavior, Oh My Tmux integration, and recovery notes are in the [full guide](docs/guide.md).

Useful places to start:

- [tmux interface and keys](docs/guide.md#tmux-interface)
- [local and remote targets](docs/guide.md#targets)
- [session restoration](docs/guide.md#session-restoration)
- [development and tests](docs/guide.md#development)

## Development

tmux-atelier uses Rust 2024 and requires Rust 1.89 or newer to build.

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
./tests/install
```

## License

[MIT](LICENSE)
