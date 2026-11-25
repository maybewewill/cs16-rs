# syntax=docker/dockerfile:1.7

ARG RUST_IMAGE=rust:1.100-slim-bookworm
ARG DEBIAN_IMAGE=debian:bookworm-slim

FROM ${RUST_IMAGE} AS builder

WORKDIR /src

# Copy workspace manifests and source crates for reproducible locked builds.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY xtask ./xtask

RUN cargo build --locked --release -p cs-server --bin cs-server \
    && strip /src/target/release/cs-server

FROM ${DEBIAN_IMAGE} AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --home-dir /home/cs16 --shell /usr/sbin/nologin cs16 \
    && mkdir -p /app/web /gamedir \
    && chown -R cs16:cs16 /app

COPY --from=builder /src/target/release/cs-server /usr/local/bin/cs-server
COPY --chown=cs16:cs16 web /app/web

ENV CS16_GAMEDIR=/gamedir \
    RUST_LOG=info

WORKDIR /app
USER cs16

EXPOSE 27015/udp
EXPOSE 27016/tcp

ENTRYPOINT ["/usr/local/bin/cs-server"]
CMD [ \
    "--proto48", \
    "--gamedir", "/gamedir", \
    "--port", "27015", \
    "--web-port", "27016", \
    "--web-dir", "/app/web", \
    "--map", "de_dust2" \
]
