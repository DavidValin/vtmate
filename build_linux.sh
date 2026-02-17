name: Build AI-Mate

on:
  push:
    branches: [main]
  pull_request:

jobs:
  # ---------------------------
  # macOS build
  # ---------------------------
  build_macos:
    name: macOS Build
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v4

      - name: Setup Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
          profile: minimal
          override: true

      - name: Cache Cargo
        uses: actions/cache@v3
        with:
          path: ~/.cargo/registry
          key: cargo-registry-${{ runner.os }}-${{ hashFiles('**/Cargo.lock') }}
          restore-keys: cargo-registry-${{ runner.os }}-

      - name: Build macOS
        run: |
          chmod +x build_macos.sh
          ./build_macos.sh --skip-package

      - name: Upload macOS artifacts
        uses: actions/upload-artifact@v3
        with:
          name: macos-artifacts
          path: dist/*

  # ---------------------------
  # Linux amd64 build
  # ---------------------------
  build_linux_amd64:
    name: Linux AMD64 Build
    runs-on: ubuntu-latest
    strategy:
      matrix:
        arch: [amd64]
    steps:
      - uses: actions/checkout@v4

      - name: Setup QEMU (optional for cross builds)
        uses: docker/setup-qemu-action@v2
        with:
          platforms: all

      - name: Setup Docker Buildx
        uses: docker/setup-buildx-action@v2

      - name: Setup Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
          profile: minimal
          override: true

      - name: Cache Cargo
        uses: actions/cache@v3
        with:
          path: ~/.cargo/registry
          key: cargo-registry-linux-amd64-${{ hashFiles('**/Cargo.lock') }}
          restore-keys: cargo-registry-linux-amd64-

      - name: Build Linux AMD64
        run: |
          chmod +x build_linux.sh
          ./build_linux.sh --arch amd64 --cache

      - name: Upload Linux AMD64 artifacts
        uses: actions/upload-artifact@v3
        with:
          name: linux-amd64-artifacts
          path: dist/*

  # ---------------------------
  # Linux arm64 build
  # ---------------------------
  build_linux_arm64:
    name: Linux ARM64 Build
    runs-on: ubuntu-22.04-arm64
    strategy:
      matrix:
        arch: [arm64]
    steps:
      - uses: actions/checkout@v4

      - name: Setup Docker Buildx
        uses: docker/setup-buildx-action@v2

      - name: Setup Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
          profile: minimal
          override: true

      - name: Cache Cargo
        uses: actions/cache@v3
        with:
          path: ~/.cargo/registry
          key: cargo-registry-linux-arm64-${{ hashFiles('**/Cargo.lock') }}
          restore-keys: cargo-registry-linux-arm64-

      - name: Build Linux ARM64
        run: |
          chmod +x build_linux.sh
          ./build_linux.sh --arch arm64 --cache

      - name: Upload Linux ARM64 artifacts
        uses: actions/upload-artifact@v3
        with:
          name: linux-arm64-artifacts
          path: dist/*
