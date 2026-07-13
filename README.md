# rsh

`rsh` is an experimental interactive shell that combines familiar Bash syntax
with typed, structured-data pipelines. It is built in Rust and includes a
multiline editor, job control, context-aware completion, session restoration,
local workflows, and optional AI-assisted command generation.

> `rsh` implements a broad and useful subset of Bash, but it is not yet a
> drop-in replacement for every Bash script. Keep `/bin/bash` as the interpreter
> for scripts that require exact Bash behavior.

## Highlights

- Bash-style commands, expansion, functions, arrays, redirections, traps, and
  foreground/background jobs.
- Structured JSON, YAML, TOML, XML, CSV, and NDJSON pipelines.
- Typed values, `let` bindings, closures, typed `def` functions, and reusable
  modules through `use`.
- Lazy streams (`range`, `from-ndjson`, `take`) and ordered parallel mapping
  with `par-each`.
- Interactive Emacs/Vi editing, fuzzy history search, Git-aware prompts,
  completions for common developer tools, bookmarks, directory frecency, and
  parameterized workflows.
- Optional OpenAI, Anthropic, or local Ollama integration. AI is disabled until
  explicitly enabled.

## Install

Build the current checkout:

```sh
cargo build --release
./target/release/rsh --version
```

Or install it into Cargo's binary directory:

```sh
cargo install --path .
```

The default build includes HTTP and AI-provider support. To build the shell core
without its HTTP client dependency:

```sh
cargo build --release --no-default-features
```

## Five-minute tour

Run an interactive shell:

```sh
rsh
```

Execute a command or a script:

```sh
rsh -c 'printf "hello %s\n" world'
rsh ./script.rsh one two
printf 'echo from-stdin\n' | rsh
```

Use structured data without a chain of text parsers:

```sh
rsh -c 'echo '\''[{"name":"Ada","age":36},{"name":"Lin","age":28}]'\'' \
  | from-json | where age -gt 30 | select name | to-table'
```

Files are decoded from their extension and can be converted on save:

```sh
rsh -c 'open users.json | where {|row| [ $row.active = true ]} | save active.yaml'
```

Typed functions and lazy pipelines extend the shell language:

```sh
rsh -c 'def add a:int b:int {|a,b| $a + $b}; add 3 4'
rsh -c 'range 1..1000000 | take 5 | each {|n| $n * $n} | to-json'
```

Discover the available commands from inside rsh:

```sh
help
help where
help --record where
```

## Command line

```text
rsh [OPTIONS] [SCRIPT [ARG ...]]
rsh [OPTIONS] -c COMMAND [NAME [ARG ...]]
```

Important options:

- `-c, --command COMMAND` executes a command string. As in Bash, the following
  `NAME` becomes `$0`, and later values become `$1`, `$2`, and so on.
- `-s, --stdin` reads a program from standard input.
- `-n, --noexec, --check` parses input without executing it.
- `-i, --interactive` requires an interactive terminal editor and cannot be
  combined with syntax-check mode or an explicit command, script, or stdin.
- `--norc` skips the interactive startup file.
- `--rcfile FILE` selects an explicit interactive startup file.
- `--session ID` restores and persists a named interactive terminal session.
- `--help` and `--version` report the binary's interface and version.

Startup and session options are accepted for command-line consistency but take
effect only when rsh starts its interactive editor; they do not alter `-c`,
script, stdin, or syntax-check execution.

CLI errors and syntax errors exit with status `2`. Command-not-found and
missing-script failures use `127`; commands or scripts that cannot be
executed or read use `126`.

## Startup and persistent state

Interactive shells import `~/.bashrc` by default for compatibility. Use
`--rcfile ~/.rshrc` for a native rsh startup file, or `--norc` for a clean
session. Non-interactive `-c`, script, and stdin execution do not implicitly
load interactive configuration or write interactive history.

History is stored at `~/.rsh_history`; named session snapshots live under
`~/.rsh/sessions`. New files are written with private permissions. History uses
a newline-safe JSONL format while retaining compatibility with the previous
tab-separated format. Session snapshots exclude process-specific variables and
names that look like credentials, tokens, passwords, or secrets.

## AI, explicitly opt-in

AI integration is opt-in. Select a provider when starting rsh:

```sh
RSH_AI_PROVIDER=ollama rsh
RSH_AI_PROVIDER=openai rsh
RSH_AI_PROVIDER=anthropic rsh
```

For cloud providers, inject `OPENAI_API_KEY` or `ANTHROPIC_API_KEY` beforehand
through your normal secret manager or protected environment configuration; do
not type secrets directly into a recorded command line. `RSH_AI_MODEL` and
`RSH_AI_BASE_URL` override provider defaults. Requests include your prompt,
OS, and current-directory path. Cloud requests do not additionally include
recent history or Git status unless
`RSH_AI_SHARE_CONTEXT=1` is set. Generated commands are suggestions: inspect
them before execution, especially when they contain destructive operations.

## Completion and workflows

Built-in completion specifications cover Git, Cargo, npm, Docker, and kubectl.
Additional JSON specs can be placed in `~/.rsh/completions/`. Local workflow
definitions live in `~/.rsh/workflows/`; press `Ctrl-G` in the editor to search
the workflow registry and fill its parameters.

## Development

The main verification commands are:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked
cargo test --all-features --locked
cargo test --no-default-features --locked
cargo build --release --all-features --locked
```

Benchmarks are available through `cargo bench` and the comparison scripts
`bench.sh` and `bench_nu.sh`.

## Current compatibility boundaries

- Startup-file import can transfer environment variables, aliases, and selected
  shell options, but not every arbitrary Bash function or interactive plugin.
- Some advanced Bash options and edge cases remain incomplete. Prefer an
  explicit Bash shebang for production scripts that depend on exact Bash
  parsing or `set -e` corner cases.
- Structured pipeline commands are rsh extensions and are not portable to Bash.
- HTTP and AI features are available only in builds with the `ai` Cargo feature.

Please include the smallest reproducing command, expected status, actual status,
and platform details when reporting a compatibility issue.
