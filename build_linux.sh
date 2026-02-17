name: Build AI-Mate

on:
  push:
    branches: [main]
  pull_request:

jobs:
  build:
    strategy:
      matrix:
        include:
          - os: ubuntu-latest
            arch: amd64
          - os: ubuntu-latest
            arch: arm64
          - os: macos-latest
            arch: native
          - os: windows-latest
            arch: x64
          - os: windows-latest
            arch: arm64

    runs-on: ${{ matrix.os }}

    env:
      DIST_DIR: dist

    steps:
      - name: Checkout repository
        uses: actions/checkout@v4

      # Cache Rust builds
      - name: Cache Rust dependencies
        uses: actions/cache@v3
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
          key: cargo-${{ runner.os }}-${{ hashFiles('**/Cargo.lock') }}

      # Cache Docker layers for faster Linux builds
      - name: Cache Docker
        if: startsWith(matrix.os, 'ubuntu')
        uses: actions/cache@v3
        with:
          path: /tmp/.docker-cache
          key: docker-${{ runner.os }}-${{ matrix.arch }}-${{ hashFiles('**/build_linux.sh') }}
          restore-keys: |
            docker-${{ runner.os }}-${{ matrix.arch }}-

      # Setup QEMU for cross-arch Docker
      - name: Setup QEMU
        if: startsWith(matrix.os, 'ubuntu')
        uses: docker/setup-qemu-action@v2

      # Build Linux
      - name: Build Linux
        if: matrix.os == 'ubuntu-latest'
        shell: bash
        run: |
          ARCH=${{ matrix.arch }}
          echo "Building Linux $ARCH"
          curl -L -o build_linux.sh https://raw.githubusercontent.com/DavidValin/ai-mate/refs/heads/main/build_linux.sh
          chmod +x build_linux.sh
          ./build_linux.sh --arch $ARCH --cache

      # Build macOS
      - name: Build macOS
        if: matrix.os == 'macos-latest'
        shell: bash
        run: |
          curl -L -o build_macos.sh https://raw.githubusercontent.com/DavidValin/ai-mate/refs/heads/main/build_macos.sh
          chmod +x build_macos.sh
          ./build_macos.sh --skip-package=false

      # Build Windows
      - name: Build Windows
        if: matrix.os == 'windows-latest'
        shell: cmd
        env:
          ARCH: ${{ matrix.arch }}
        run: |
          curl -L -o build_windows.bat https://raw.githubusercontent.com/DavidValin/ai-mate/refs/heads/main/build_windows.bat
          set WIN_WITH_VULKAN=0
          build_windows.bat --arch %ARCH%

      # Upload artifacts
      - name: Upload build artifacts
        uses: actions/upload-artifact@v4
        with:
          name: ai-mate-${{ matrix.os }}-${{ matrix.arch }}
          path: dist
