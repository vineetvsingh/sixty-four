# The whole site in one container: the built pages plus the server that hosts
# online games. Works on any host that runs a Dockerfile (Fly.io, Render,
# Railway, Cloud Run, a VPS).
#
#   docker build -t sixtyfour .
#   docker run -p 8080:8080 sixtyfour

FROM rust:1-bookworm AS build
# Keep this version the same as the wasm-bindgen crate in Cargo.lock.
RUN rustup target add wasm32-unknown-unknown \
 && cargo install wasm-bindgen-cli --version 0.2.130 --locked
WORKDIR /src
COPY . .
RUN sh build.sh && cargo build --release -p sixtyfour-server

FROM debian:bookworm-slim
WORKDIR /srv
COPY --from=build /src/target/release/serve /usr/local/bin/serve
COPY --from=build /src/dist ./dist
# Listen on every interface, not just localhost. Hosts that choose the port set PORT themselves.
ENV HOST=0.0.0.0 PORT=8080
EXPOSE 8080
USER nobody
CMD ["serve"]
