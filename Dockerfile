# syntax=docker/dockerfile:1
# One image with every LMOxide binary. docker-compose.yml runs `lmoxide` (game server, API and
# patch server in one process, docker/lmoxide.example.toml); the separate programs are there too.
#   docker build -t lmoxide .
#   docker build --target dist --output type=local,dest=out .   (the release tarball's files, in out/)

FROM rust:1-bookworm AS build
RUN apt-get update \
 && apt-get install -y --no-install-recommends protobuf-compiler cmake clang \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
# what /v1/version reports (.git isn't copied in): the release workflow and
# docker-compose.build.yml pass the commit and its unix time
ARG LMOXIDE_GIT_COMMIT=""
ARG LMOXIDE_GIT_COMMIT_TIME=""
ENV LMOXIDE_GIT_COMMIT=$LMOXIDE_GIT_COMMIT LMOXIDE_GIT_COMMIT_TIME=$LMOXIDE_GIT_COMMIT_TIME
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked \
      -p lmoxide -p lmoxide-server -p lmoxide-api -p lmoxide-patch-server -p lmoxide-patcher \
 && mkdir -p /out/bin \
 && cp target/release/lmoxide target/release/lmoxide-server target/release/lmoxide-api \
       target/release/lmoxide-patch-server target/release/lmoxide-patcher /out/bin/
# Linux client overlay (the client kit's prebuilt libraries and launcher), served to patchers
RUN mkdir -p /out/linux-overlay \
 && cc -shared -fPIC -O2 -o /out/linux-overlay/libgconf-2.so.4 patches/linux-client/gconf-stub.c \
 && cc -shared -fPIC -O2 -o /out/linux-overlay/liblmofixes.so patches/linux-client/lmo-fixes.c -lpthread \
 && cp patches/linux-client/lmo /out/linux-overlay/lmo

# What the release tarball holds (.github/workflows/release.yml)
FROM scratch AS dist
COPY --from=build /out/bin/ /bin/
COPY --from=build /out/linux-overlay/ /linux-overlay/
COPY --from=build /src/content/ /content/
COPY --from=build /src/data/monster_data.json /src/data/effect_catalog.json /src/data/stats.json /reference/
COPY --from=build /src/docker/lmoxide.example.toml /src/README.md /

# git: the world editor's content working copy is a git clone (docs/HOSTING.md)
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates tini git \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home-dir /data lmoxide \
 && mkdir -p /data /patch-data /clients \
 && chown lmoxide:lmoxide /data /patch-data
COPY --from=build /out/bin/ /usr/local/bin/
COPY --from=build /out/linux-overlay/ /usr/local/share/lmoxide/linux-overlay/
COPY --from=build /src/content/ /usr/local/share/lmoxide/content/
# the world editor's catalogs (looks, effect packages, stats)
COPY --from=build /src/data/monster_data.json /src/data/effect_catalog.json /src/data/stats.json /usr/local/share/lmoxide/reference/
USER lmoxide
WORKDIR /data
ENV LMOXIDE_DATA_DIR=/data LMOXIDE_CONTENT_DIR=/usr/local/share/lmoxide/content \
    LMOXIDE_REFERENCE_DIR=/usr/local/share/lmoxide/reference RUST_LOG=info
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["lmoxide", "--config", "/etc/lmoxide/lmoxide.toml"]
