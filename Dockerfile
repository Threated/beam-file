FROM lukemathwalker/cargo-chef:latest-rust-bookworm AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder 
COPY --from=planner /app/recipe.json recipe.json
# Build dependencies - this is the caching Docker layer!
RUN cargo chef cook --release --recipe-path recipe.json
# Build application
COPY . .
ENV RUSTFLAGS='-C target-feature=+crt-static'
RUN cargo build --release --bin beam-file --all-features --target x86_64-unknown-linux-gnu

FROM scratch
STOPSIGNAL SIGINT
COPY --from=builder /app/target/x86_64-unknown-linux-gnu/release/beam-file /usr/local/bin/
ENTRYPOINT ["/usr/local/bin/beam-file"]
