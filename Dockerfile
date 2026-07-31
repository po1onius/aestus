FROM node:26.3.0-bookworm-slim AS web-builder
WORKDIR /app/web

COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web ./
RUN npm run build

FROM rust:1.96.0-bookworm AS srv-builder
WORKDIR /app/srv

RUN apt-get update \
    && apt-get install -y --no-install-recommends libpq-dev pkg-config cmake clang \
    && rm -rf /var/lib/apt/lists/*

COPY srv/Cargo.toml srv/Cargo.lock ./
COPY srv/src ./src
# Wasmtime 的 Component bindgen 在编译期读取 WIT 契约；该目录必须与源码一起进入
# builder stage，否则宏展开失败，并进一步产生绑定模块类型缺失的连锁错误。
COPY srv/wit ./wit
RUN cargo build --release

FROM debian:bookworm-slim AS diesel-cli-downloader

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl xz-utils \
    && rm -rf /var/lib/apt/lists/*

ARG TARGETARCH
ARG DIESEL_CLI_VERSION=2.3.10
RUN set -eux; \
    target_arch="${TARGETARCH:-$(dpkg --print-architecture)}"; \
    case "${target_arch}" in \
        amd64) diesel_arch="x86_64" ;; \
        arm64) diesel_arch="aarch64" ;; \
        *) echo "unsupported Docker target architecture: ${target_arch}" >&2; exit 1 ;; \
    esac; \
    artifact="diesel_cli-${diesel_arch}-unknown-linux-gnu.tar.xz"; \
    base_url="https://github.com/diesel-rs/diesel/releases/download/v${DIESEL_CLI_VERSION}"; \
    curl -fsSL "${base_url}/${artifact}" -o "/tmp/${artifact}"; \
    curl -fsSL "${base_url}/${artifact}.sha256" -o "/tmp/${artifact}.sha256"; \
    cd /tmp; \
    sha256sum -c "${artifact}.sha256"; \
    mkdir -p /opt/diesel-cli; \
    tar -xJf "${artifact}" -C /opt/diesel-cli; \
    diesel_path="$(find /opt/diesel-cli -type f -name diesel -print -quit)"; \
    test -n "${diesel_path}"; \
    install -m 0755 "${diesel_path}" /usr/local/bin/diesel

FROM debian:bookworm-slim AS diesel-cli
WORKDIR /app/srv

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libpq5 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=diesel-cli-downloader /usr/local/bin/diesel /usr/local/bin/diesel

ENTRYPOINT ["diesel"]

FROM debian:bookworm-slim AS runtime
WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libpq5 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=srv-builder /app/srv/target/release/aestus /usr/local/bin/aestus-gateway
COPY --from=web-builder /app/web/dist /app/web/dist

# 监听地址、日志目录与静态资源目录属于镜像运行约定，Compose 不重复暴露这些内部路径配置。
ENV AESTUS_BIND_ADDR=0.0.0.0:8080 \
    AESTUS_LOG_DIRECTORY=/app/logs \
    AESTUS_WEB_DIST_DIR=/app/web/dist
EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/aestus-gateway"]
