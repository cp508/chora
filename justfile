@_default:
    just --list --unsorted

# fmt + lint
check:
    cargo fmt --all -- --check
    cargo clippy --all-targets --all-features -- -D warnings

# just check + auto tests
qa: check
    cargo test

# build debug
debug:
    cargo build

# build release
release:
    cargo build --release
