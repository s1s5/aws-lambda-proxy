# ------------- build ----------------
FROM s1s5/muslrust:1.90.0-stable-2025-10-29 AS builder

RUN groupadd -g 999 app && \
    useradd -d /app -s /bin/bash -u 999 -g 999 app

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN --mount=type=cache,id=aws-lambda-proxy,target=/rust/target \
    --mount=type=cache,target=/opt/cargo/registry \
    --mount=type=cache,target=/opt/cargo/git \
    cargo build --release --bin aws-lambda-proxy && \
    cp ./target/x86_64-unknown-linux-musl/release/aws-lambda-proxy /aws-lambda-proxy

# ------------- server ----------------
FROM scratch AS backend

ENV RUST_LOG=info,sqlx::query=error

COPY --from=builder /etc/passwd /etc/passwd
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt

COPY --from=builder /aws-lambda-proxy /aws-lambda-proxy

USER 999
EXPOSE 8000
ENTRYPOINT [ "/aws-lambda-proxy" ]

