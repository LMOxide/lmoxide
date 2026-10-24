# syntax=docker/dockerfile:1
# One image with every LMOxide server binary; docker-compose.yml picks the command per service.
#   docker build -t lmoxide .

FROM rust:1-bookworm AS build
RUN apt-get update \
 && apt-get install -y --no-install-recommends protobuf-compiler cmake clang \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked \
      -p lmoxide-server -p lmoxide-api -p lmoxide-patch-server -p lmoxide-patcher \
 && mkdir -p /out/bin \
 && cp target/release/lmoxide-server target/release/lmoxide-api \
       target/release/lmoxide-patch-server target/release/lmoxide-patcher /out/bin/
# Linux client overlay (the client kit's prebuilt libraries and launcher), served to patchers
RUN mkdir -p /out/linux-overlay \
 && cc -shared -fPIC -O2 -o /out/linux-overlay/libgconf-2.so.4 patches/linux-client/gconf-stub.c \
 && cc -shared -fPIC -O2 -o /out/linux-overlay/liblmofixes.so patches/linux-client/lmo-fixes.c -lpthread \
 && cp patches/linux-client/lmo /out/linux-overlay/lmo

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates tini \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home-dir /data lmoxide \
 && mkdir -p /data /patch-data /clients \
 && chown lmoxide:lmoxide /data /patch-data
COPY --from=build /out/bin/ /usr/local/bin/
COPY --from=build /out/linux-overlay/ /usr/local/share/lmoxide/linux-overlay/
USER lmoxide
WORKDIR /data
ENV LMOXIDE_DATA_DIR=/data RUST_LOG=info
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["lmoxide-server"]
