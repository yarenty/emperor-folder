# emperor-folder: MCP server confined to one folder, over stdio.
#
#   docker build -t emperor-folder .
#   docker run -i --rm -v "$PWD:/data" emperor-folder                 # read-only
#   docker run -i --rm -v "$PWD:/data" emperor-folder --root /data --writable

FROM rust:1.88-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked --bin emperor-folder

FROM debian:bookworm-slim
LABEL org.opencontainers.image.title="emperor-folder" \
      org.opencontainers.image.description="MCP server confined to one folder: list, read, search, stat, write, edit and move files, never outside the root" \
      org.opencontainers.image.source="https://github.com/yarenty/emperor-folder" \
      org.opencontainers.image.url="https://github.com/yarenty/emperor-folder" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"
RUN useradd --system --uid 10001 --no-create-home --shell /usr/sbin/nologin folder \
    && mkdir /data && chown folder:folder /data
COPY --from=build /src/target/release/emperor-folder /usr/local/bin/emperor-folder
USER folder
WORKDIR /data
ENTRYPOINT ["emperor-folder", "serve", "--stdio"]
CMD ["--root", "/data"]
