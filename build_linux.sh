name: Build AI-Mate

on:
  push:
    branches: [main, ci-test]
  pull_request:

jobs:
  build:
    strategy:
      matrix:
        include:
          # Linux builds
          - os: ubuntu-latest
            arch: amd64
          - os: ubuntu-latest
            arch: arm64

          # macOS build
          - os: macos-latest
            arch: native

          # Windows builds
          - os: windows-latest
            arch: x64
          - os: windows-latest
            arch: arm64

    runs-on: ${{ matrix.os }}

    steps:
      - name: Checkout repository
        uses: actions/checkout@v4

      # -------------------------
      # Linux build
      # -------------------------
      - name: Build Linux
        if: matrix.os == 'ubuntu-latest'
        shell: bash
        run: |
          ARCH=${{ matrix.arch }}
          echo "Building Linux $ARCH"
          curl -L -o build_linux.sh https://raw.githubusercontent.com/DavidValin/ai-mate/refs/heads/main/build_linux.sh
          chmod +x build_linux.sh
          ./build_linux.sh --arch $ARCH

      # -------------------------
      # macOS build
      # -------------------------
      - name: Build macOS
        if: matrix.os == 'macos-latest'
        shell: bash
        run: |
          echo "Building macOS"
          curl -L -o build_macos.sh https://raw.githubusercontent.com/DavidValin/ai-mate/refs/heads/main/build_macos.sh
          chmod +x build_macos.sh
          ./build_macos.sh

      # -------------------------
      # Windows build
      # -------------------------
      - name: Build Windows
        if: matrix.os == 'windows-latest'
        shell: cmd
        env:
          ARCH: ${{ matrix.arch }}
        run: |
          echo Building Windows %ARCH%
          curl -L -o build_windows.bat https://raw.githubusercontent.com/DavidValin/ai-mate/refs/heads/main/build_windows.bat
          set WIN_WITH_VULKAN=0
          build_windows.bat --arch %ARCH%

      # -------------------------
      # Upload artifacts
      # -------------------------
      - name: Upload artifacts
        uses: actions/upload-artifact@v4
        with:
          name: ai-mate-${{ matrix.os }}-${{ matrix.arch }}
          path: dist
