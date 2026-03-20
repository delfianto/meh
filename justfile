# Default recipe: list available commands
default:
    @just --list

# Run the application (debug mode)
run *ARGS:
    cargo run -- {{ARGS}}

# Lint, format check, and run tests
check:
    cargo fmt --check
    cargo clippy -- -D warnings
    cargo test

# Check then build (debug)
build: check
    cargo build

# Check then build (release)
release: check
    cargo build --release

# Install binary to ~/.local/bin
install: release
    mkdir -p ~/.local/bin
    cp target/release/meh ~/.local/bin/meh
    @echo "Installed meh to ~/.local/bin/meh"

# Auto-format code
fmt:
    cargo fmt

# Run tests only
test *ARGS:
    cargo test {{ARGS}}

# Run clippy only
lint:
    cargo clippy -- -D warnings

# Clean build artifacts
clean:
    cargo clean
