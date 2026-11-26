# Release build image for the portable Linux packages (.tar.gz, AppImage).
#
# Ubuntu 22.04 has glibc 2.35: a binary linked here runs on that and every
# newer distribution (Arch and CachyOS included), which a binary linked on
# Arch, with the newest glibc, would not. Source and target folder come in
# through volumes (see ../build.sh).
FROM ubuntu:22.04

ARG RUST_TOOLCHAIN=1.99.1
ARG DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl git pkg-config clang cmake ninja-build \
        python3 python3-pil rsync file desktop-file-utils zsync \
        libfontconfig1-dev libfreetype-dev \
        libx11-dev libxcursor-dev libxrandr-dev libxi-dev libxcb1-dev \
        libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan-dev \
    && rm -rf /var/lib/apt/lists/*

ENV CARGO_HOME=/cargo \
    RUSTUP_HOME=/rustup \
    PATH=/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --no-modify-path --profile minimal --default-toolchain "$RUST_TOOLCHAIN"

# appimagetool and the AppImage runtime it packs, both from the AppImage
# project's releases. They are AppImages themselves: run them with
# APPIMAGE_EXTRACT_AND_RUN=1, since a container has no FUSE.
RUN mkdir -p /opt/appimage \
    && curl -fsSL -o /opt/appimage/appimagetool \
        https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage \
    && curl -fsSL -o /opt/appimage/runtime-x86_64 \
        https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-x86_64 \
    && chmod +x /opt/appimage/appimagetool
ENV APPIMAGE_EXTRACT_AND_RUN=1

WORKDIR /src
