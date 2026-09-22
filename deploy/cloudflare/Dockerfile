FROM rust:1.98-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY deploy/relay/removent-relay.service ./deploy/relay/removent-relay.service
RUN cargo build --locked --release -p removent-relay

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --create-home relay
COPY --from=build /src/target/release/removent-relay /usr/local/bin/removent-relay
USER 10001
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/removent-relay", "serve-container"]
