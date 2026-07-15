# Aether — Pre-built binary runtime image
# Binary must be built locally before docker build:
#   CARGO_HOME=$HOME/.cargo cargo build --release
FROM docker.io/library/debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY target/release/aether ./
COPY config.toml.example ./config.toml

ENV AETHER_HOST=0.0.0.0
ENV AETHER_PORT=8000
ENV RUST_LOG=info

EXPOSE 8000

ENTRYPOINT ["./aether"]
