name: Build AI-Mate

on:
  push:
    tags:
      - 'v*'
  workflow_dispatch:

jobs:

  build_macos:
    name: macOS Build
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
          override: true

      - name: Build macOS
        run: ./build_macos.sh --cache

      - name: Upload macOS artifacts
        uses: actions/upload-artifact@v4
        with:
          name: ai-mate-macos
          path: dist/packages/*.tar.gz

  build_linux_amd64:
    name: Linux AMD64 Build
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
          override: true

      - name: Build Linux AMD64
        run: ./build_linux.sh --arch amd64 --cache

      - name: Upload Linux AMD64 artifacts
        uses: actions/upload-artifact@v4
        with:
          name: ai-mate-linux-amd64
          path: dist/packages/*.tar.gz

  build_linux_arm64:
    name: Linux ARM64 Build
    runs-on: ubuntu-22.04-arm64
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
          override: true

      - name: Build Linux ARM64
        run: ./build_linux.sh --arch arm64 --cache

      - name: Upload Linux ARM64 artifacts
        uses: actions/upload-artifact@v4
        with:
          name: ai-mate-linux-arm64
          path: dist/packages/*.tar.gz

  build_windows:
    name: Windows Build
    runs-on: windows-latest
    strategy:
      matrix:
        arch: [x86_64, aarch64]
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
          override: true
          target: ${{ matrix.arch == 'x86_64' && 'x86_64-pc-windows-gnu' || 'aarch64-pc-windows-msvc' }}

      - name: Build Windows
        run: |
          echo "Building for ${{ matrix.arch }}"
          if [ "${{ matrix.arch }}" == "x86_64" ]; then
            cargo build --release --target x86_64-pc-windows-gnu
          else
            cargo build --release --target aarch64-pc-windows-msvc
          fi

      - name: Package Windows artifact
        run: |
          mkdir -p dist/packages
          if [ "${{ matrix.arch }}" == "x86_64" ]; then
            cp target/x86_64-pc-windows-gnu/release/ai-mate.exe dist/packages/ai-mate-windows-x64.exe
          else
            cp target/aarch64-pc-windows-msvc/release/ai-mate.exe dist/packages/ai-mate-windows-arm64.exe
          fi

      - name: Upload Windows artifacts
        uses: actions/upload-artifact@v4
        with:
          name: ai-mate-windows-${{ matrix.arch }}
          path: dist/packages/*.exe