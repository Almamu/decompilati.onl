FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/decompsite /usr/local/bin/decompsite
# Point DATABASE_URL at your PostgreSQL server, e.g. postgres://user:pass@db/decompsite
ENV BIND=0.0.0.0:8080 DATA_DIR=/data
VOLUME /data
EXPOSE 8080
CMD ["decompsite"]
