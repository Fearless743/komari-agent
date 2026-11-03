# syntax=docker/dockerfile:1

# ---------------------------------------------------------------------------
# Build stage
# ---------------------------------------------------------------------------
FROM rust:1.95-bookworm AS builder

WORKDIR /src

# aws-lc-sys (pulled in by rustls) builds C code through CMake.
RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake perl \
    && rm -rf /var/lib/apt/lists/*

# Version metadata is injected at compile time by crates/agent-cli/src/lib.rs.
ARG KOMARI_AGENT_VERSION=dev
ARG KOMARI_AGENT_VERSION_HASH=
ENV KOMARI_AGENT_VERSION=${KOMARI_AGENT_VERSION} \
    KOMARI_AGENT_VERSION_HASH=${KOMARI_AGENT_VERSION_HASH}

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

# Build only the agent binary; the runtime stage keeps the final image small.
RUN cargo build --release --locked -p agent-cli \
    && install -Dm755 target/release/komari-agent /out/komari-agent

# ---------------------------------------------------------------------------
# Runtime stage
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim

LABEL org.opencontainers.image.title="komari-agent" \
      org.opencontainers.image.description="Komari monitoring agent" \
      org.opencontainers.image.source="https://github.com/fearless743/komari-agent" \
      org.opencontainers.image.licenses="MIT"

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 komari \
    && useradd --system --uid 10001 --gid komari \
        --home-dir /app --shell /usr/sbin/nologin komari \
    && mkdir -p /app /etc/komari \
    && chown -R komari:komari /app /etc/komari

COPY --from=builder /out/komari-agent /app/komari-agent

# The agent writes auto-discovery.json next to its executable and may replace
# the executable during self-update, so /app belongs to the runtime user.
WORKDIR /app
USER 10001:10001

# Configuration volume. The only state the agent writes is auto-discovery.json,
# which is created next to the executable in /app; see readme.md for details.
VOLUME ["/etc/komari"]

ENTRYPOINT ["/app/komari-agent"]
CMD ["--help"]
