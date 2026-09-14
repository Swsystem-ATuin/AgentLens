# syntax=docker/dockerfile:1

FROM rust:1.90-bookworm AS builder

WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY templates ./templates
COPY assets ./assets
COPY schema.sql ./

RUN cargo build --locked --release

FROM debian:bookworm-slim AS runtime

LABEL org.opencontainers.image.source="https://git.s4w.uk/infrastructure/agent-lens.git"
LABEL org.opencontainers.image.licenses="GPL-3.0-or-later"

RUN groupadd --gid 10001 agent-lens \
    && useradd --uid 10001 --gid agent-lens \
        --home-dir /var/lib/agent-lens --no-create-home \
        --shell /usr/sbin/nologin agent-lens \
    && install -d -o agent-lens -g agent-lens /var/lib/agent-lens

COPY --from=builder /build/target/release/agent-lens /usr/local/bin/agent-lens

USER agent-lens
WORKDIR /var/lib/agent-lens

ENV AGENT_LENS_DB=/var/lib/agent-lens/agent-lens.sqlite \
    AGENT_LENS_STATE_DB=/var/lib/agent-lens/agent-lens-state.sqlite

EXPOSE 3001

ENTRYPOINT ["agent-lens"]
CMD ["serve", "--bind", "0.0.0.0:3001"]
