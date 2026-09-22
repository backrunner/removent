FROM rust:1.98-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY deploy/relay/removent-relay.service ./deploy/relay/removent-relay.service
RUN cargo build --locked --release -p removent-relay

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --create-home relay && mkdir -p /var/lib/removent-relay && chown relay /var/lib/removent-relay
COPY --from=build /src/target/release/removent-relay /usr/local/bin/removent-relay
USER 10001
EXPOSE 48700/udp
VOLUME ["/var/lib/removent-relay"]
ENTRYPOINT ["/usr/local/bin/removent-relay"]
CMD ["serve", "/etc/removent-relay/server.toml"]
