FROM rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 sss \
    && useradd --uid 10001 --gid sss --no-create-home --shell /usr/sbin/nologin sss \
    && install -d -o sss -g sss -m 0700 /var/lib/sss
COPY --from=build /src/target/release/sss /usr/local/bin/sss
USER 10001:10001
VOLUME ["/var/lib/sss"]
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/sss"]
CMD ["serve", "--listen", "0.0.0.0:8080", "--data-dir", "/var/lib/sss", "--public-url", "http://localhost:8080"]
