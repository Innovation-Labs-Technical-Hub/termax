# Termax developer tasks. Run `just` to list them.
# `make <recipe>` forwards to the same recipes (see Makefile).

set shell := ["bash", "-euo", "pipefail", "-c"]

# Scratch output (ignored by git): perf results and the demo vault.
out_dir := justfile_directory() / "target"
demo_dir := out_dir / "demo"
perf_dir := out_dir / "perf"
# Fixed demo credentials, only ever used for the throwaway demo vault.
demo_password := "termax-demo-password"
msrv := `sed -n 's/^rust-version = "\(.*\)"/\1/p' Cargo.toml | head -1`
zig_version := "0.15.2"
openspec_version := "1.17.0"
# The Ghostty backend (default on Linux) only builds on Linux for now; see README.
crates := if os() == "linux" { "--workspace" } else { "--workspace --exclude termax-term-ghostty --exclude termax-term-ghostty-sys" }

# List recipes
default:
    @just --list --unsorted

# ── Toolchains ──────────────────────────────────────────────────────────────

# Show versions of every tool the project uses
doctor:
    @echo "rustc:  $(rustc --version)"
    @echo "cargo:  $(cargo --version)"
    @echo "MSRV:   {{msrv}} (workspace rust-version)"
    @echo "clippy: $(cargo clippy --version 2>/dev/null || echo 'missing: rustup component add clippy')"
    @echo "fmt:    $(cargo fmt --version 2>/dev/null || echo 'missing: rustup component add rustfmt')"
    @echo "deny:   $(cargo deny --version 2>/dev/null || echo 'missing: just setup')"
    @echo "sshd:   $( (/usr/sbin/sshd -V 2>&1 || true) | grep -m1 OpenSSH || echo 'missing: SSH interop tests will skip')"
    @echo "zig:    $(zig version 2>/dev/null || echo 'missing: required on Linux for the Ghostty terminal backend (just zig-install)')"

# Install rustup components, the MSRV toolchain, cargo-deny and Zig
setup: zig-install
    rustup component add rustfmt clippy
    rustup toolchain install {{msrv}} --profile minimal
    cargo install --locked cargo-deny

# Install the pinned Zig into ~/.local, verified against ziglang.org's SHA-256
zig-install:
    #!/usr/bin/env bash
    set -euo pipefail
    ver={{zig_version}}
    if zig version 2>/dev/null | grep -qx "$ver"; then echo "zig $ver already installed"; exit 0; fi
    key="$(uname -m)-$(uname -s | tr '[:upper:]' '[:lower:]')"
    meta=$(curl -fsSL https://ziglang.org/download/index.json)
    field() { echo "$meta" | python3 -c "import sys,json;print(json.load(sys.stdin)['$ver']['$key']['$1'])"; }
    url=$(field tarball); sum=$(field shasum)
    tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
    curl -fsSL "$url" -o "$tmp/zig.tar.xz"
    echo "$sum  $tmp/zig.tar.xz" | sha256sum -c -
    mkdir -p ~/.local/zig-$ver ~/.local/bin
    tar -xJf "$tmp/zig.tar.xz" -C ~/.local/zig-$ver --strip-components=1
    ln -sf ~/.local/zig-$ver/zig ~/.local/bin/zig
    echo "installed zig $(~/.local/bin/zig version); make sure ~/.local/bin is on PATH"

# ── Everyday checks ─────────────────────────────────────────────────────────

# Build the workspace (on Linux, Zig must be on PATH for the Ghostty backend)
build:
    cargo build {{crates}}

# Format all code
fmt:
    cargo fmt --all

# Lint with warnings as errors (same as CI)
lint:
    cargo fmt --all --check
    cargo clippy {{crates}} --all-targets -- -D warnings

# Run all tests, or one crate's: `just test termax-ssh`
test crate="":
    cargo test {{ if crate == "" { crates } else { "-p " + crate } }}

# Run only the SSH tests against a throwaway local OpenSSH sshd
test-ssh:
    cargo test -p termax-ssh --test sshd -- --nocapture

# Licence and advisory checks (same as CI)
deny:
    cargo deny check

# Check the build on the minimum supported Rust version
msrv:
    cargo +{{msrv}} check {{crates}} --all-targets

# Everything CI runs on every platform, in order
ci: lint test deny

# Build and open the API docs
doc:
    cargo doc {{crates}} --no-deps --open

# Remove build output, perf results and the demo vault
clean:
    cargo clean

# ── Terminal backends ───────────────────────────────────────────────────────

# Test the TermCore backends against their shared expectations (Ghostty on Linux only)
term-test:
    cargo test -p termax-term {{ if os() == "linux" { "-p termax-term-ghostty" } else { "" } }}

# Terminal numbers for Ghostty and alacritty, written side by side as JSON (Linux)
[linux]
compare-backends:
    mkdir -p "{{perf_dir}}"
    cargo xtask perf --no-ssh --json "{{perf_dir}}/ghostty.json"
    cargo xtask perf --no-ssh --backend alacritty --json "{{perf_dir}}/alacritty.json"

# ── Performance ─────────────────────────────────────────────────────────────

# Run the perf harness: `just perf --no-ssh`, `just perf --backend alacritty`
perf *args:
    cargo xtask perf {{args}}

# Save this machine's numbers as the local baseline
perf-baseline:
    mkdir -p "{{perf_dir}}"
    cargo xtask perf --json "{{perf_dir}}/baseline.json"

# Compare against the local baseline; fails on >10% regressions
perf-compare *args:
    cargo xtask perf --baseline "{{perf_dir}}/baseline.json" {{args}}

# ── Specs (OpenSpec) ────────────────────────────────────────────────────────

# Run the pinned OpenSpec CLI: `just openspec list --specs`
openspec *args:
    bunx @fission-ai/openspec@{{openspec_version}} {{args}}

# Validate every spec and change strictly (same as CI)
spec-validate:
    bunx @fission-ai/openspec@{{openspec_version}} validate --all --strict

# List capability specs and active changes
spec-list:
    bunx @fission-ai/openspec@{{openspec_version}} list --specs
    bunx @fission-ai/openspec@{{openspec_version}} list

# Re-import specs from the design set's BRD YAML (review the diff; edited specs are overwritten)
spec-import brd_yaml_dir:
    python3 tools/openspec_from_brd.py "{{brd_yaml_dir}}" openspec/specs
    just spec-validate

# ── Playground ──────────────────────────────────────────────────────────────

# Run the tmx CLI: `just tmx policy check "rm -rf /"`
tmx *args:
    cargo run -q -p termax-cli -- {{args}}

# Ask the agent policy about a command: `just policy "sudo rm -rf /" prod`
policy command env="none":
    -cargo run -q -p termax-cli -- policy check --env {{env}} {{quote(command)}}

# Create a throwaway demo vault and import your ~/.ssh/config and known_hosts
demo-init:
    rm -rf "{{demo_dir}}" && mkdir -p "{{demo_dir}}"
    TMX_VAULT_PASSWORD='{{demo_password}}' \
      cargo run -q -p termax-cli -- --db "{{demo_dir}}/vault.db" --json vault init --kdf-memory-kib 1024 \
      | sed -n 's/.*"secret_key": "\(.*\)".*/\1/p' > "{{demo_dir}}/secret-key"
    @echo "Demo vault: {{demo_dir}}/vault.db (demo-only password and Secret Key)"
    -just demo import ssh-config
    -just demo import known-hosts

# Run any tmx command against the demo vault: `just demo hosts list`
demo *args:
    TMX_VAULT_PASSWORD='{{demo_password}}' TMX_SECRET_KEY="$(cat '{{demo_dir}}/secret-key')" \
      cargo run -q -p termax-cli -- --db "{{demo_dir}}/vault.db" {{args}}

# Start a throwaway OpenSSH sshd and print how to connect (Ctrl-C stops it)
sshd:
    cargo run -q -p xtask -- sshd
