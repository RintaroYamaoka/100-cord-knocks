# 100本ノックの実行イメージ — 7 言語のツールチェーンを 1 枚に焼く。
#
# 本番 (Vercel Sandbox) と検証 (ローカル Docker) が**同じイメージ**を使うためのもの。
# 別々の実行環境を持っていた時代は「verifier は緑なのに本番は TS2550」が構造的に起きた。
#
# 版はすべて 2026-08-29 に検証済みの組み合わせに固定する (ADR 0002 の表)。
# 版を上げるときは 2100 問の再検証 (cargo run -p verifier) を通してから。
#
#   g++ 13.4 / javac 22.0.2 / dotnet SDK 6.0.428 / python 3.13 / node 20 / tsc 5.6.2 / rust stable
#
# レイヤーはツールチェーンごとに分ける (VCR は圧縮後 1 レイヤー 500MB / 合計 15GB が上限)。

FROM gcc:13 AS gcc13
FROM node:20-bookworm-slim AS node20

# python:3.13-slim を土台にする (python 本体とその共有ライブラリが揃っているため)
FROM python:3.13-slim-bookworm

# g++ が要るリンカ・ヘッダと、dotnet が要る ICU / OpenSSL
RUN apt-get update && apt-get install -y --no-install-recommends \
      binutils libc6-dev make ca-certificates curl xz-utils \
      libicu72 libssl3 zlib1g \
    && rm -rf /var/lib/apt/lists/*

# --- C++ (gcc 13.4) ---
COPY --from=gcc13 /usr/local /usr/local
# rustc のリンカ探索は `cc` を見る。gcc イメージには gcc/g++ しか無いので張る
RUN ln -sf /usr/local/bin/gcc /usr/local/bin/cc
ENV LD_LIBRARY_PATH=/usr/local/lib64:/usr/local/lib

# --- JavaScript (node 20) / TypeScript (tsc 5.6.2) ---
COPY --from=node20 /usr/local /usr/local
RUN npm install -g typescript@5.6.2 && npm cache clean --force

# --- Java (Temurin 22.0.2) ---
ENV JAVA_HOME=/opt/java/jdk
RUN mkdir -p /opt/java/jdk \
    && curl -fsSL "https://api.adoptium.net/v3/binary/version/jdk-22.0.2%2B9/linux/x64/jdk/hotspot/normal/eclipse" \
       -o /tmp/jdk.tar.gz \
    && tar -xzf /tmp/jdk.tar.gz -C /opt/java/jdk --strip-components=1 \
    && rm /tmp/jdk.tar.gz
ENV PATH=$JAVA_HOME/bin:$PATH

# --- C# (dotnet SDK 6.0.428) ---
ENV DOTNET_ROOT=/usr/share/dotnet \
    DOTNET_CLI_TELEMETRY_OPTOUT=1 \
    DOTNET_NOLOGO=1 \
    DOTNET_SKIP_FIRST_TIME_EXPERIENCE=1
RUN curl -fsSL https://dot.net/v1/dotnet-install.sh -o /tmp/dotnet-install.sh \
    && bash /tmp/dotnet-install.sh --version 6.0.428 --install-dir "$DOTNET_ROOT" \
    && rm /tmp/dotnet-install.sh
ENV PATH=$DOTNET_ROOT:$PATH

# --- Rust (stable / edition 2024) ---
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo
RUN curl -fsSL https://sh.rustup.rs -o /tmp/rustup.sh \
    && sh /tmp/rustup.sh -y --no-modify-path --profile minimal --default-toolchain stable \
    && rm /tmp/rustup.sh \
    && chmod -R a+rX "$RUSTUP_HOME" "$CARGO_HOME"
ENV PATH=/usr/local/cargo/bin:$PATH

# --- ひな形プロジェクト (shared::runner::TEMPLATE_ROOT と対応) ---
# Rust と C# は単一ファイルでは動かない。restore / 初回ビルドをイメージに焼き込み、
# 実行時はコピーして使う (サンドボックスは外向き通信を閉じているので restore できない)。
RUN mkdir -p /opt/knocks/rust/src \
    && printf '%s\n' \
        '[package]' 'name = "knocks"' 'version = "0.0.0"' 'edition = "2024"' \
        '' '[lib]' 'path = "src/lib.rs"' \
        > /opt/knocks/rust/Cargo.toml \
    && echo 'pub fn placeholder() {}' > /opt/knocks/rust/src/lib.rs \
    && (cd /opt/knocks/rust && cargo test --offline >/dev/null 2>&1) \
    && chmod -R a+rX /opt/knocks/rust

RUN dotnet new console -n knocks -o /opt/knocks/csharp \
    && (cd /opt/knocks/csharp && dotnet build -v q --nologo -o /tmp/_warm >/dev/null 2>&1) \
    && rm -rf /tmp/_warm \
    && chmod -R a+rX /opt/knocks/csharp

# 版の記録。実行時に `cat /opt/knocks/VERSIONS` で本番の版が確認できる。
RUN { echo "gcc:      $(g++ --version | head -1)"; \
      echo "javac:    $(javac -version 2>&1)"; \
      echo "dotnet:   $(dotnet --version)"; \
      echo "python:   $(python3 --version)"; \
      echo "node:     $(node --version)"; \
      echo "tsc:      $(tsc --version)"; \
      echo "rustc:    $(rustc --version)"; \
    } > /opt/knocks/VERSIONS && cat /opt/knocks/VERSIONS
