FROM rust:1.91-bullseye AS builder

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends protobuf-compiler ca-certificates git \
    && rm -rf /var/lib/apt/lists/*

# Use git CLI for dependencies; libgit2 occasionally fails on tagged git deps in containers
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

COPY . .
RUN cargo build --release --locked

FROM debian:bookworm-slim AS runtime

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd -r appuser && useradd -r -g appuser appuser

COPY --from=builder /app/target/release/quorum-list-server /usr/local/bin/quorum-list-server
USER appuser

EXPOSE 3000

ENV API_HOST=0.0.0.0
ENV API_PORT=3000

CMD ["quorum-list-server"]
