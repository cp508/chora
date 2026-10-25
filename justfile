@_default:
    just --list --unsorted

# fmt + lint (full CLI build + no-default library for mobile)
check:
    cargo fmt --all -- --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo clippy --lib --no-default-features -- -D warnings

# just check + auto tests
qa: check
    cargo test
    cargo test --lib --no-default-features

# build debug
debug:
    cargo build

# build release
release:
    cargo build --release

# move release to /usr/bin
install_local: release
    sudo cp -f ./target/release/chora /usr/bin/chora

# apply completions: fish sources, bash evals, other: nothing
completions:
    case "$SHELL" in \
        *fish*) "$SHELL" -c 'chora completion fish | source';; \
        *bash*) "$SHELL" -c 'eval "$(chora completion bash)"';; \
    esac

# publish to crates.io, token from sops/api.yaml (never in history/env files)
cargo_deploy: qa
    #!/usr/bin/env bash
    set -euo pipefail
    export CARGO_REGISTRY_TOKEN="$(sops decrypt --extract '["cargo_api"]' sops/api.yaml)"
    cargo publish
