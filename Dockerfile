# syntax=docker/dockerfile:1
# One image for `web` (`ged serve`) and `worker` (`ged worker`).
# The Rails image stays in Dockerfile.rails and is selected with
# `GED_STACK=rails bin/worktree-stack up` until T16.

FROM rust:1.99.1-bookworm AS build

WORKDIR /src
COPY rust /src/rust
COPY docs/api/openapi.yaml /src/docs/api/openapi.yaml
WORKDIR /src/rust
RUN cargo build --release --locked -p ged

FROM debian:bookworm-slim

RUN apt-get update -qq && \
    apt-get install --no-install-recommends -y curl ca-certificates postgresql-client && \
    rm -rf /var/lib/apt/lists/* && \
    groupadd --system --gid 1000 ged && \
    useradd ged --uid 1000 --gid 1000 --create-home --shell /bin/bash && \
    mkdir -p /rails/storage/staging && \
    chown -R ged:ged /rails

WORKDIR /rails
COPY --from=build /src/rust/target/release/ged /usr/local/bin/ged
COPY bin/ged-docker-entrypoint /usr/local/bin/ged-docker-entrypoint
RUN chmod 755 /usr/local/bin/ged-docker-entrypoint

USER 1000:1000
ENV PORT=80 \
    RAILS_ENV=production
EXPOSE 80
ENTRYPOINT ["/usr/local/bin/ged-docker-entrypoint"]
CMD ["ged", "serve"]
