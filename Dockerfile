FROM rust:bullseye as builder

WORKDIR /drone

COPY . /drone

RUN cargo build --release

FROM debian:bullseye-slim AS runner

COPY --from=builder /drone/target/release/drone /drone

EXPOSE 3000

CMD ["./drone"]
