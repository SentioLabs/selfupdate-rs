# selfupdate

Synchronous self-update with release channels for Rust CLIs publishing GitHub
releases. A Rust port of [Sentio Labs' go-selfupdate](https://github.com/sentiolabs/go-selfupdate),
retaining its MIT license and attribution. This library uses Rust 2024.

```no_run
use selfupdate::{CancellationToken, GitHubSource, ScriptInstaller, UpdateOptions, Updater};

# fn main() -> selfupdate::Result<()> {
let mut updater = Updater::builder("mytool", env!("CARGO_PKG_VERSION"),
    GitHubSource::new("acme", "mytool")?)
    .installer(ScriptInstaller::new(
        "https://raw.githubusercontent.com/acme/mytool/main/scripts/install.sh"))
    .build();
updater.update(&CancellationToken::new(), UpdateOptions { check: true, ..Default::default() })?;
# Ok(())
# }
```

Add `selfupdate = "0.1"` to Cargo.toml. Optional command integration uses
`selfupdate = { version = "0.1", features = ["clap"] }`.

The crate is published as `selfupdate`; its GitHub repository remains
[`SentioLabs/selfupdate-rs`](https://github.com/SentioLabs/selfupdate-rs).
Version 0.1.0 was published under the earlier package name `selfupdate-rs`;
releases from 0.1.1 use `selfupdate`.

## Workflows

`Updater::builder` requires a `Source`. Add a `Store` for persistent channel
selection and an `Installer` for installation. These traits are object-safe;
consumer failures can use `Error::external`, retaining the original error.
`MemStore` holds a channel in memory; `FuncStore` adapts config read/save closures.
Without a store, the channel is stable and switching returns `Error::NoStore`.
Without an installer, checks and declined updates work; an accepted installation
returns `Error::NoInstaller` before running the hook.

`update` checks first. `check` prints status only, even with `force`.
`force` permits reinstalling or downgrading. Otherwise equal/newer current
versions print status and return. `yes` skips confirmation; only a trimmed `y`
or `Y` line accepts. EOF and input errors decline. A pre-install hook runs after
confirmation, before installation; its failure prevents installation.

Status/prompt input, output, and warning output are injectable with `input`,
`output`, and `error_output`. Output writes and flushes propagate failures.
Persistence happens before its success message, so an output error can follow
a successful save. These streams are separate from the installer's process stdio.

## Channels and versions

Default specifications are stable, rc (`v1.2.3-rc.1` or `v1.2.3-rc1`), and nightly
(`v1.2.3-nightly.20260903`). Custom `Channel` names and regex-based `ChannelSpec`s
are supported. A specification with no pattern uses `/releases/latest`.
Channel switches print their configured warning and prompt unless `yes`; a
channel with no warning switches directly. Stable has no warning by default.

Patterned channels examine one page of 100 releases, preserving newest-first
order and filtering drafts. The first matching tag competes with the first stable
release by version precedence. If no stable appears on that page, `/releases/latest`
is always consulted, even when no tag matches. Empty or prerelease fallback
results are ignored; fallback request errors propagate. There is no pagination.

`compare(current, latest)` returns `Ordering::Greater` when an update is available.
It follows [Go x/mod/semver precedence](https://pkg.go.dev/golang.org/x/mod/semver):
invalid versions compare equal and below valid versions; build metadata is ignored;
shortened `v1` and `v1.2` versions and arbitrarily large numeric components work.
`normalize_version` trims whitespace, adds `v`, and maps empty/`dev` to `v0.0.0-dev`.
Release-to-release selection compares original tags, as Go does; publish `v` tags.
Dotted `rc.10` sorts above `rc.9`, while undotted `rc10` sorts below `rc9`.
Tag nightlies with the *next* version so they sort above the existing stable release.

## HTTP and runtime requirements

`GitHubSource::builder` accepts an API base (including enterprise paths), bearer
token, blocking Reqwest client, and nonzero per-request timeout (default 30 seconds).
Every request sets GitHub's JSON Accept header and a `selfupdate/<version>`
User-Agent, including when using a supplied client. Timeouts include response body
reads. See [Reqwest's blocking client configuration](https://docs.rs/reqwest/latest/reqwest/blocking/struct.ClientBuilder.html).
Use this synchronous library outside an async runtime; an async application should
construct, use, and drop it on a blocking thread.

The core and optional Clap integration support Linux, macOS, and Windows.
`ScriptInstaller` supports **Linux and macOS only** and requires `bash` and `curl`
on PATH. Other platforms return `Error::UnsupportedPlatform`; provide your own
installer there. The configured URL must serve a trusted install script accepting
`--force --tag=<tag>`. Bash runs with `pipefail`, so failed downloads fail installation.
URL and validated tag are passed as quoted positional arguments to fixed shell
code. `ProcessStdio` supports inherited streams, the null device, or file handles;
file handles can also capture output or supply a terminal.

## Cancellation

Clone `CancellationToken` and call `cancel()` from another thread. Tokens are
checked around lookups, after HTTP headers/body reads, and before output,
persistence, hooks, and process creation. Consumer sources/installers/hooks must
cooperate during their own work. Cancellation cannot undo completed side effects.
Active HTTP requests finish or hit their timeout; blocking prompt reads cannot be
interrupted and are checked when the read returns.

With nonterminal stdin, the installer creates a separate process group, polls every
25 ms, and cancels the group with SIGTERM, escalating to SIGKILL after five seconds
if the group remains. It reaps the direct child and checks descendants even if
Bash exits first. Processes that deliberately leave the group are outside its control.
With terminal stdin, it retains the caller's group so sudo and terminal Ctrl-C work.
Token cancellation stops only the direct child in this mode; use terminal Ctrl-C
to interrupt the full foreground pipeline. It never signals the caller's group.

## Clap embedding and examples

The `clap` feature exposes `clap::command`, standalone update/channel builders,
and `clap::dispatch` for a parent's matches or the `self` matches. It adds:

```text
mytool self update [--check] [--force/-f] [--yes/-y]
mytool self channel [stable|rc|nightly] [--yes/-y]
```

Channel choices come from your specifications. `--check` has no short flag by
default, leaving a parent's `-c` available. Opt in with `CommandOptions { check_short:
Some('c') }` only when that shorthand is free. Callers own parsing/help and use the
updater's streams for dispatch output. `dispatch` returns false for unrelated commands.

Compiling examples in `examples/` cover `basic`, `config_store`, `pre_install`,
`cancellation`, and `clap` (the last requires `--features clap`). The network examples
use placeholder repositories and check-only mode; they never replace a running binary.

## Differences from Go

Rust uses a required-source builder, typed errors, `Ordering`, borrowed/owned
adapters, and explicit cancellation tokens. Output errors propagate. An incomplete
confirmation line at EOF declines, including a bare `y` without a newline.
The stable fallback also runs when there is no matching channel release. Installer
URLs are never interpolated into shell source, and cancellation escalates for the
whole detached group. HTTP requests have a bounded default timeout. Script
installation is explicitly limited to Linux/macOS.

## Verification

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo test --all-features
cargo package
cargo package --all-features
```

Tests use fakes, local HTTP servers, harmless temporary scripts, and a Unix PTY;
they never replace the test binary. CI runs these checks on Linux/macOS and core,
HTTP, and Clap tests on Windows.

## Releases and crates.io

The workflows follow [worktrunk-wip's CI/CD setup](https://github.com/bfirestone/worktrunk-wip/tree/c4ca00f8e5ba9e962b427d93bd903df0de57a89c/.github/workflows):
Release Please prepares Rust version/changelog PRs from conventional commits on
`main`. Merging a release PR creates the tag and GitHub release. The release
workflow then runs the full CI matrix against that tag, tests both feature sets
in release mode, checks that the tag matches `Cargo.toml`, and publishes the crate
using crates.io Trusted Publishing (OIDC).

Publishing runs as a dependent job in `release.yaml`, because tags created with
`GITHUB_TOKEN` do not start separate workflows. The same limitation affects
bot-created PRs, so the release workflow explicitly dispatches `ci.yaml` on the
release PR branch. Normal PRs and pushes to `main` run CI automatically. Rust
builds use `Swatinem/rust-cache`, and Linux CI also runs the template's typo check.

Before enabling releases in the hosting repository:

1. Allow GitHub Actions to create pull requests in the repository's Actions
   settings. The workflow declares its required token permissions.
2. Establish ownership of `selfupdate` on crates.io. If the crate is new,
   complete the initial authenticated publish before configuring its trusted
   publisher; confirm the crate name is available first.
3. In the crate's Trusted Publishing settings, enter owner **`SentioLabs`**,
   repository **`selfupdate-rs`**, and workflow filename **`release.yaml`**. This
   workflow does not specify a GitHub environment, so leave that restriction unset. See the
   [crates.io setup documentation](https://crates.io/docs/trusted-publishing).

The publish job obtains a short-lived token with
`rust-lang/crates-io-auth-action@v1`; no long-lived crates.io secret is needed for
subsequent releases. If publishing fails after the tag is created, fix the setup
and rerun the failed jobs in that original release run. A new push alone will not
re-publish an already-created release. Registry versions cannot be overwritten.
