# CI image: Rust runs here, built with docker buildx on GitHub-hosted runners (ubuntu-24.04 for
# x86_64, ubuntu-24.04-arm for the native arm64 stages).
# The crate has no dependencies and no private git deps, so no step needs a token. The only fetch
# is the fuzz crate's own dependency (libfuzzer-sys from crates.io); every later step runs offline.
# Every tool is installed in a stage before `COPY . .`, so a new commit does not install it again,
# and the fetch runs from the manifests and lockfiles alone.
FROM rust:1.99.1-bookworm AS toolchain
ARG TARGETARCH
# On x86_64, gcc-aarch64-linux-gnu is the linker of `cargo build --target aarch64-unknown-linux-gnu`
# (the aarch64 lint and link check in `ci`). On arm64 that target is the host: no cross toolchain.
RUN rustup component add rustfmt clippy && rustup target add aarch64-unknown-linux-gnu \
    && if [ "$TARGETARCH" = amd64 ]; then \
         apt-get update && apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu libc6-dev-arm64-cross \
         && rm -rf /var/lib/apt/lists/* \
         && printf '[target.aarch64-unknown-linux-gnu]\nlinker = "aarch64-linux-gnu-gcc"\n' >> "$CARGO_HOME/config.toml"; \
       fi
WORKDIR /src
# The manifests and lockfiles of the crate and of the fuzz crate, with stub targets, so the fetch
# is a layer that only a change to them rebuilds. The fuzz crate depends on the crate by path.
COPY Cargo.toml Cargo.lock ./
COPY tests/fuzz/Cargo.toml tests/fuzz/Cargo.lock tests/fuzz/
RUN mkdir -p src tests/fuzz/fuzz_targets && touch src/lib.rs \
    && for t in parse read boxes iloc iinf iref ipma ipco derived tmap movie movie_read mini transform; do touch tests/fuzz/fuzz_targets/$t.rs; done \
    && cargo fetch --locked --manifest-path tests/fuzz/Cargo.toml \
    && rm -rf src tests/fuzz/fuzz_targets

FROM toolchain AS src
ENV CARGO_NET_OFFLINE=true
COPY . .

# The checks run in the build, so a red check fails the build; a rerun of the same source is a
# BuildKit cache hit. The target dir is a cache mount, so it persists across builds. The target
# cache is shared by every branch and cargo compares mtimes: another branch's newer build would be
# reused as this one's, so every RUN touches the .rs files itself, just before cargo starts.
FROM src AS ci
RUN --mount=type=cache,id=isobmff-rs-target-ci,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + && cargo fmt --check
RUN --mount=type=cache,id=isobmff-rs-target-ci,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + && cargo clippy --locked --all-targets -- -D warnings
# Lint the aarch64 target too: cfg(target_arch) code is not compiled for x86_64.
RUN --mount=type=cache,id=isobmff-rs-target-ci,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + && cargo clippy --locked --all-targets --target aarch64-unknown-linux-gnu -- -D warnings
RUN --mount=type=cache,id=isobmff-rs-target-ci,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + && cargo test --locked --no-fail-fast
RUN --mount=type=cache,id=isobmff-rs-target-ci,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + && RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps
# The fuzz crate is its own workspace, so the root checks above skip it.
RUN find src examples tests -name '*.rs' -exec touch {} + \
    && cargo fmt --manifest-path tests/fuzz/Cargo.toml --check \
    && cargo check --locked --manifest-path tests/fuzz/Cargo.toml
# The examples are built too, so the cross linker runs (a lib alone is not linked).
RUN --mount=type=cache,id=isobmff-rs-target-ci,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + && cargo build --locked --release --target aarch64-unknown-linux-gnu --examples

# The arm64 stages are built natively on an arm64 runner (ubuntu-24.04-arm): no emulator.
FROM toolchain AS native-arm64
ENV CARGO_NET_OFFLINE=true
COPY . .

# The library tests on aarch64. The parser is portable (no SIMD), so one run is enough; the rest
# of the suite runs in `ci`.
FROM native-arm64 AS arm64
RUN --mount=type=cache,id=isobmff-rs-target-arm64,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + \
    && cargo test --locked --target aarch64-unknown-linux-gnu --lib

# libheif v1.23.7 (LGPL-3.0) and libavif v1.4.2 (BSD-2-Clause) oracles, CI only, never linked into
# the crate. Both are built from git at the pinned commit, static, with no codecs. The library
# builds come first (libavif with the experimental `mini` box reader on, libheif reads `mini` always);
# tests/parity/c is copied after them, so a change to the .c files does not
# rebuild the libraries.
FROM debian:bookworm AS c-oracle
RUN apt-get update && apt-get install -y --no-install-recommends build-essential cmake git ca-certificates pkg-config \
    && rm -rf /var/lib/apt/lists/*
RUN git clone -q --depth 1 --branch v1.23.7 https://github.com/strukturag/libheif /src/libheif \
    && test "$(git -C /src/libheif rev-parse HEAD)" = 12a259a5882dd99d45ec5fc77e296a3b620d1e2e \
    && cmake -S /src/libheif -B /b/heif -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/opt/oracle \
       -DBUILD_SHARED_LIBS=OFF -DENABLE_PLUGIN_LOADING=OFF -DBUILD_TESTING=OFF -DBUILD_DOCUMENTATION=OFF \
       -DWITH_LIBDE265=OFF -DWITH_X265=OFF -DWITH_X264=OFF -DWITH_OpenH264_DECODER=OFF -DWITH_KVAZAAR=OFF \
       -DWITH_AOM_DECODER=OFF -DWITH_AOM_ENCODER=OFF -DWITH_DAV1D=OFF -DWITH_RAV1E=OFF -DWITH_SvtEnc=OFF \
       -DWITH_JPEG_DECODER=OFF -DWITH_JPEG_ENCODER=OFF -DWITH_OpenJPEG_DECODER=OFF -DWITH_OpenJPEG_ENCODER=OFF \
       -DWITH_OPENJPH_ENCODER=OFF -DWITH_FFMPEG_DECODER=OFF -DWITH_UVG266=OFF -DWITH_VVDEC=OFF -DWITH_VVENC=OFF \
       -DWITH_UNCOMPRESSED_CODEC=ON -DWITH_HEADER_COMPRESSION=OFF -DWITH_LIBSHARPYUV=OFF \
       -DWITH_EXAMPLES=OFF -DWITH_GDK_PIXBUF=OFF \
    && cmake --build /b/heif -j"$(nproc)" && cmake --install /b/heif \
    && rm -rf /src/libheif /b/heif
RUN git clone -q --depth 1 --branch v1.4.2 https://github.com/AOMediaCodec/libavif /src/libavif \
    && test "$(git -C /src/libavif rev-parse HEAD)" = c5240fc79fe5c2407e10afd35f5505ef6333ea49 \
    && cmake -S /src/libavif -B /b/avif -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/opt/oracle -DBUILD_SHARED_LIBS=OFF \
       -DAVIF_CODEC_AOM=OFF -DAVIF_CODEC_DAV1D=OFF -DAVIF_CODEC_LIBGAV1=OFF -DAVIF_CODEC_RAV1E=OFF -DAVIF_CODEC_SVT=OFF \
       -DAVIF_CODEC_AVM=OFF -DAVIF_LIBYUV=OFF -DAVIF_LIBSHARPYUV=OFF -DAVIF_LIBXML2=OFF -DAVIF_ENABLE_EXPERIMENTAL_MINI=ON \
       -DAVIF_BUILD_APPS=OFF -DAVIF_BUILD_TESTS=OFF \
    && cmake --build /b/avif -j"$(nproc)" && cmake --install /b/avif \
    && rm -rf /src/libavif /b/avif
ENV PKG_CONFIG_PATH=/opt/oracle/lib/pkgconfig:/opt/oracle/lib/x86_64-linux-gnu/pkgconfig
COPY tests/parity/c /c
# libheif is C++: the C oracle is compiled with gcc and linked with g++.
RUN gcc -O2 -std=gnu99 -Wall -c -o /tmp/heif_oracle.o /c/heif_oracle.c $(pkg-config --cflags libheif) \
    && g++ -o /usr/local/bin/heif-oracle /tmp/heif_oracle.o -static-libstdc++ -static-libgcc $(pkg-config --static --libs libheif) \
    && gcc -O2 -std=gnu99 -Wall -c -o /tmp/avif_oracle.o /c/avif_oracle.c $(pkg-config --cflags libavif) \
    && g++ -o /usr/local/bin/avif-oracle /tmp/avif_oracle.o -static-libstdc++ -static-libgcc $(pkg-config --static --libs libavif)

# libheif v1.23.7 again (same commit), for the transform oracle only: with the HEVC decoder libde265
# and the AV1 decoder dav1d from Debian bookworm, and the uncompressed codec, in its own prefix
# /opt/oracle-tr. The no-codec libheif of `c-oracle` and its outputs are not changed. Only
# transform_oracle.c is copied (a change to the other .c files does not rebuild this stage).
# The Debian -dev packages ship static libraries; the runtime packages are installed in
# `transform-tools` in case the link falls back to the shared ones.
FROM debian:bookworm AS c-oracle-tr
RUN apt-get update && apt-get install -y --no-install-recommends build-essential cmake git ca-certificates pkg-config libde265-dev libdav1d-dev \
    && rm -rf /var/lib/apt/lists/*
RUN git clone -q --depth 1 --branch v1.23.7 https://github.com/strukturag/libheif /src/libheif \
    && test "$(git -C /src/libheif rev-parse HEAD)" = 12a259a5882dd99d45ec5fc77e296a3b620d1e2e \
    && cmake -S /src/libheif -B /b/heif -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/opt/oracle-tr \
       -DBUILD_SHARED_LIBS=OFF -DENABLE_PLUGIN_LOADING=OFF -DBUILD_TESTING=OFF -DBUILD_DOCUMENTATION=OFF \
       -DWITH_LIBDE265=ON -DWITH_DAV1D=ON -DWITH_X265=OFF -DWITH_X264=OFF -DWITH_OpenH264_DECODER=OFF -DWITH_KVAZAAR=OFF \
       -DWITH_AOM_DECODER=OFF -DWITH_AOM_ENCODER=OFF -DWITH_RAV1E=OFF -DWITH_SvtEnc=OFF \
       -DWITH_JPEG_DECODER=OFF -DWITH_JPEG_ENCODER=OFF -DWITH_OpenJPEG_DECODER=OFF -DWITH_OpenJPEG_ENCODER=OFF \
       -DWITH_OPENJPH_ENCODER=OFF -DWITH_FFMPEG_DECODER=OFF -DWITH_UVG266=OFF -DWITH_VVDEC=OFF -DWITH_VVENC=OFF \
       -DWITH_UNCOMPRESSED_CODEC=ON -DWITH_HEADER_COMPRESSION=OFF -DWITH_LIBSHARPYUV=OFF \
       -DWITH_EXAMPLES=OFF -DWITH_GDK_PIXBUF=OFF \
    && cmake --build /b/heif -j"$(nproc)" && cmake --install /b/heif \
    && rm -rf /src/libheif /b/heif
ENV PKG_CONFIG_PATH=/opt/oracle-tr/lib/pkgconfig:/opt/oracle-tr/lib/x86_64-linux-gnu/pkgconfig
COPY tests/parity/c/transform_oracle.c /c/transform_oracle.c
RUN gcc -O2 -std=gnu99 -Wall -c -o /tmp/transform_oracle.o /c/transform_oracle.c $(pkg-config --cflags libheif) \
    && g++ -o /usr/local/bin/heif-transform-oracle /tmp/transform_oracle.o -static-libstdc++ -static-libgcc $(pkg-config --static --libs libheif)

# The public corpus (tests/parity/corpus.toml: blobless sparse checkouts at pinned commits) in a
# cache mount, so a build downloads it once. sharing=locked: two builds never download at once.
# Only the list of the source variants of this corpus.toml (fetch.sh: v/<key>/<name>, written once,
# never changed) reaches the layer, so branches with different corpus.toml files share the cache
# without seeing each other's files; the stages that read the corpus mount the same cache
# and depend on this stage through that list. git-lfs is for the LFS sources.
FROM debian:bookworm-slim AS data
RUN apt-get update && apt-get install -y --no-install-recommends git git-lfs ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY tests/parity/fetch.sh tests/parity/corpus.toml /parity/
RUN --mount=type=cache,id=isobmff-rs-data,target=/data,sharing=locked \
    sh /parity/fetch.sh /parity/corpus.toml /data /data.list \
    && cat /data.list

# The reference trees of isobmff-rs on the libheif and libavif test data (x86_64, one thread):
# what the aarch64 oracle must reproduce byte for byte.
FROM src AS x86-ref
COPY --from=data /data.list /data.list
RUN --mount=type=cache,id=isobmff-rs-target-parity,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + \
    && cargo build --locked --release --example oracle && cp target/release/examples/oracle /usr/local/bin/isobmff-oracle
RUN --mount=type=cache,id=isobmff-rs-data,target=/data,sharing=locked,readonly \
    sh tests/parity/run.sh ref /ref

# ffprobe (Debian bookworm ffmpeg, 5.1.x) and gawk, for gate 5 of tests/parity/run.sh: the MP4 tracks
# of isobmff-rs are compared with the packets that ffprobe reads. The pinned ffmpeg is used when
# bookworm still has it, else the current one; summary.md prints the version either way. gawk -M
# (64-bit integers) is checked here, so a gawk without it fails the build, not the gate.
FROM toolchain AS parity-tools
RUN apt-get update && apt-get install -y --no-install-recommends gawk \
    && { apt-get install -y --no-install-recommends ffmpeg=7:5.1.9-0+deb12u1 || apt-get install -y --no-install-recommends ffmpeg; } \
    && ffprobe -version | head -n 1 \
    && test "$(gawk -M 'BEGIN { print 9223372036854775807 + 1 }')" = 9223372036854775808 \
    && rm -rf /var/lib/apt/lists/*

FROM parity-tools AS parity-src
ENV CARGO_NET_OFFLINE=true
COPY . .

# Parity gates (tests/parity/run.sh full): libheif and libavif vs isobmff-rs on the corpus, the
# mutations, --threads 1 vs default, --read vs slice, ffprobe vs the MP4 tracks. The stage does not fail on a diff: the report
# (summary.md, *.diff, status) is the build output (target parity-report), and the workflow fails
# on `status`, as the report is wanted most when the gate is red.
FROM parity-src AS parity
COPY --from=c-oracle /usr/local/bin/heif-oracle /usr/local/bin/heif-oracle
COPY --from=c-oracle /usr/local/bin/avif-oracle /usr/local/bin/avif-oracle
COPY --from=data /data.list /data.list
RUN --mount=type=cache,id=isobmff-rs-target-parity,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + \
    && cargo build --locked --release --example oracle && cp target/release/examples/oracle /usr/local/bin/isobmff-oracle
RUN --mount=type=cache,id=isobmff-rs-data,target=/data,sharing=locked,readonly \
    mkdir -p /report && { sh tests/parity/run.sh full /report; echo $? > /report/status; }

FROM scratch AS parity-report
COPY --from=parity /report /

# Transform oracle (tests/parity/transform.sh): libheif with libde265, dav1d and the unci codec
# against isobmff::apply_transforms, on the corpus and on generated unci files. Same layout as the
# parity stage: the stage never fails on a diff, the report (summary.md, transform.diff, status) is
# the build output (target transform-report), and the workflow fails on `status`. The tools come
# before `COPY . .`; the runtime packages of libde265 and dav1d are for a link that fell back to the
# shared libraries (a no-op when the static ones were used).
FROM toolchain AS transform-tools
RUN apt-get update && apt-get install -y --no-install-recommends libde265-0 libdav1d6 \
    && rm -rf /var/lib/apt/lists/*

FROM transform-tools AS transform-src
ENV CARGO_NET_OFFLINE=true
COPY . .

FROM transform-src AS transform
COPY --from=c-oracle-tr /usr/local/bin/heif-transform-oracle /usr/local/bin/heif-transform-oracle
COPY --from=data /data.list /data.list
RUN --mount=type=cache,id=isobmff-rs-target-parity,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + \
    && cargo build --locked --release --example oracle && cp target/release/examples/oracle /usr/local/bin/isobmff-oracle
RUN --mount=type=cache,id=isobmff-rs-data,target=/data,sharing=locked,readonly \
    mkdir -p /report && { sh tests/parity/transform.sh /report; echo $? > /report/status; }

FROM scratch AS transform-report
COPY --from=transform /report /

# The x86_64 reference trees as a build output (job parity-ref uploads them for parity-arm64).
FROM scratch AS x86-ref-out
COPY --from=x86-ref /ref /

# The oracle built natively for aarch64, run on the libheif and libavif test data, compared with
# the x86_64 trees of `x86-ref-out`. The workflow passes them as the named build context `x86ref`.
FROM native-arm64 AS parity-arm64
COPY --from=data /data.list /data.list
RUN --mount=type=cache,id=isobmff-rs-target-arm64,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + \
    && cargo build --locked --release --target aarch64-unknown-linux-gnu --example oracle \
    && cp target/aarch64-unknown-linux-gnu/release/examples/oracle /usr/local/bin/isobmff-oracle-arm64
RUN --mount=type=bind,from=x86ref,target=/ref,rw \
    --mount=type=cache,id=isobmff-rs-data,target=/data,sharing=locked,readonly \
    sh tests/parity/run.sh arm /ref

# Fuzzing (fuzz.yml): nightly and cargo-fuzz, with the targets built into the image. The workflow
# runs it with `docker create` + `docker start`, so a BuildKit cache hit cannot skip the fuzzing.
FROM toolchain AS fuzz-tools
RUN rustup toolchain install nightly --profile minimal \
    && cargo install cargo-fuzz --locked --version 0.13.1
ENV CARGO_NET_OFFLINE=true

# Seeds are the first 64 KiB of every corpus file (tests/fuzz/seeds, gathered here from the data
# cache mount, not kept in git) plus tests/data/fuzz-grown if present.
FROM fuzz-tools AS fuzz
COPY . .
COPY --from=data /data.list /data.list
RUN --mount=type=cache,id=isobmff-rs-data,target=/data,sharing=locked,readonly \
    sh tests/parity/run.sh gather tests/fuzz/seeds \
    && cargo +nightly fuzz build --fuzz-dir tests/fuzz

# Benchmark (bench.yml): libheif and libavif (C oracles, bench mode) and examples/bench on the same
# corpus, in one container, so the ratio compares the same machine. BENCH_RUN changes per run, so
# a cache hit of the same source does not reuse an old timing.
FROM src AS bench
COPY --from=c-oracle /usr/local/bin/heif-oracle /usr/local/bin/heif-oracle
COPY --from=c-oracle /usr/local/bin/avif-oracle /usr/local/bin/avif-oracle
COPY --from=data /data.list /data.list
RUN --mount=type=cache,id=isobmff-rs-target-bench,sharing=locked,target=/src/target \
    find src examples tests -name '*.rs' -exec touch {} + \
    && cargo build --locked --release --example bench && cp target/release/examples/bench /usr/local/bin/isobmff-bench
ARG BENCH_RUN=0
RUN --mount=type=cache,id=isobmff-rs-data,target=/data,sharing=locked,readonly \
    echo "run $BENCH_RUN" && mkdir -p /report && { sh tests/parity/bench.sh /report; echo $? > /report/status; }

FROM scratch AS bench-report
COPY --from=bench /report /
