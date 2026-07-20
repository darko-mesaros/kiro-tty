# Kiro TTY - common tasks
#
# Run `just` (or `just --list`) to see everything.
# Docker recipes assume docker-compose.yml; connection recipes assume the
# default localhost port bindings below.

# Published ports (keep in sync with docker-compose.yml)
telnet_port := "2323"
ssh_port := "2222"

# List available recipes
default:
    @just --list

# ------------------------------------------------------------------ Rust ---

# Build the binary (debug)
build:
    cargo build

# Build the optimized release binary
release:
    cargo build --release

# Run locally with the tty agent (needs kiro-cli on PATH). Extra args pass through.
run *ARGS:
    cargo run -- --agent tty {{ARGS}}

# Run the test suite
test:
    cargo test

# Format the source
fmt:
    cargo fmt

# Lint with clippy, treating warnings as errors
lint:
    cargo clippy --all-targets -- -D warnings

# Fast type-check without producing a binary
check:
    cargo check

# Remove build artifacts
clean:
    cargo clean

# ---------------------------------------------------------------- Docker ---

# Build the container image
docker-build:
    docker compose build

# Start the container in the background
up:
    docker compose up -d

# Stop and remove the container
down:
    docker compose down

# Rebuild the image and restart the container
rebuild: down docker-build up

# Follow the container logs
logs:
    docker compose logs -f

# Show container status
ps:
    docker compose ps

# Open a root debug shell inside the running container
enter:
    docker exec -it kiro-tty bash

# ------------------------------------------------------------ Connecting ---

# Connect over telnet (the vintage path; 127.0.0.1 avoids the IPv6 retry)
telnet:
    telnet 127.0.0.1 {{telnet_port}}

# Connect over SSH
ssh:
    ssh kiro@127.0.0.1 -p {{ssh_port}}
