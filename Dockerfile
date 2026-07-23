# syntax=docker/dockerfile:1.7

FROM rust:1.96.1-bookworm AS builder

WORKDIR /app
COPY Cargo.toml ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-20260623-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/io_uring_benchmark /usr/local/bin/io_uring_benchmark

ENV BIND_ADDRESS=0.0.0.0:8080
ENV TRANSPORT=io_uring
ENV SYNC_WRITES=0

EXPOSE 8080

CMD ["io_uring_benchmark"]
