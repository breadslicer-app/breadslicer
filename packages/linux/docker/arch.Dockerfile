# Build and test image for Breadslicer on Arch Linux.
#
# Holds the toolchain and the system libraries only: the source and the
# target folder come in through volumes (see ../build.sh), so the image does
# not change when the code does.
FROM archlinux:latest

ARG RUST_TOOLCHAIN=1.99.1

RUN pacman -Syu --noconfirm --needed \
        base-devel git clang cmake ninja python curl rsync zip unzip \
        fontconfig freetype2 ttf-dejavu \
        libx11 libxcursor libxrandr libxi libxcb libxkbcommon libxkbcommon-x11 \
        wayland dbus \
        mesa vulkan-swrast vulkan-icd-loader \
        xorg-server-xvfb xorg-xauth \
        rustup \
    && pacman -Scc --noconfirm

ENV CARGO_HOME=/cargo \
    RUSTUP_HOME=/rustup \
    PATH=/cargo/bin:$PATH
RUN rustup toolchain install "$RUST_TOOLCHAIN" --profile minimal \
        --component rustfmt --component clippy \
    && rustup default "$RUST_TOOLCHAIN"

# The smoke test (../ci/smoke.sh): makepkg runs as a plain user; screenshots
# and window properties of the app under Xvfb.
RUN pacman -S --noconfirm --needed imagemagick xorg-xwininfo xorg-xprop \
    && pacman -Scc --noconfirm \
    && useradd -m builder

WORKDIR /src
