#!/usr/bin/env sh
set -eu

# Downloads a tarball from GitHub Releases and unpacks it
# into ~/.local/. If you'd prefer to do this manually, see
# https://github.com/harrisonju123/PrisM#getting-started.

main() {
    platform="$(uname -s)"
    arch="$(uname -m)"
    channel="${PRISM_CHANNEL:-stable}"
    PRISM_VERSION="${PRISM_VERSION:-latest}"
    # Use TMPDIR if available (for environments with non-standard temp directories)
    if [ -n "${TMPDIR:-}" ] && [ -d "${TMPDIR}" ]; then
        temp="$(mktemp -d "$TMPDIR/prism-XXXXXX")"
    else
        temp="$(mktemp -d "/tmp/prism-XXXXXX")"
    fi

    if [ "$platform" = "Darwin" ]; then
        platform="macos"
    elif [ "$platform" = "Linux" ]; then
        platform="linux"
    else
        echo "Unsupported platform $platform"
        exit 1
    fi

    case "$platform-$arch" in
        macos-arm64* | linux-arm64* | linux-armhf | linux-aarch64)
            arch="aarch64"
            ;;
        macos-x86* | linux-x86* | linux-i686*)
            arch="x86_64"
            ;;
        *)
            echo "Unsupported platform or architecture"
            exit 1
            ;;
    esac

    if command -v curl >/dev/null 2>&1; then
        curl () {
            command curl -fL "$@"
        }
    elif command -v wget >/dev/null 2>&1; then
        curl () {
            wget -O- "$@"
        }
    else
        echo "Could not find 'curl' or 'wget' in your path"
        exit 1
    fi

    "$platform" "$@"

    if [ "$(command -v prism)" = "$HOME/.local/bin/prism" ]; then
        echo "Prism has been installed. Run with 'prism'"
    else
        echo "To run Prism from your terminal, you must add ~/.local/bin to your PATH"
        echo "Run:"

        case "$SHELL" in
            *zsh)
                echo "   echo 'export PATH=\$HOME/.local/bin:\$PATH' >> ~/.zshrc"
                echo "   source ~/.zshrc"
                ;;
            *fish)
                echo "   fish_add_path -U $HOME/.local/bin"
                ;;
            *)
                echo "   echo 'export PATH=\$HOME/.local/bin:\$PATH' >> ~/.bashrc"
                echo "   source ~/.bashrc"
                ;;
        esac

        echo "To run Prism now, '~/.local/bin/prism'"
    fi
}

linux() {
    if [ -n "${PRISM_BUNDLE_PATH:-}" ]; then
        cp "$PRISM_BUNDLE_PATH" "$temp/prism-linux-$arch.tar.gz"
    else
        echo "Downloading Prism version: $PRISM_VERSION"
        curl "https://cloud.zed.dev/releases/$channel/$PRISM_VERSION/download?asset=zed&arch=$arch&os=linux&source=install.sh" > "$temp/prism-linux-$arch.tar.gz"
    fi

    suffix=""
    if [ "$channel" != "stable" ]; then
        suffix="-$channel"
    fi

    appid=""
    case "$channel" in
      stable)
        appid="dev.prism.Prism"
        ;;
      nightly)
        appid="dev.prism.Prism-Nightly"
        ;;
      preview)
        appid="dev.prism.Prism-Preview"
        ;;
      dev)
        appid="dev.prism.Prism-Dev"
        ;;
      *)
        echo "Unknown release channel: ${channel}. Using stable app ID."
        appid="dev.prism.Prism"
        ;;
    esac

    # Unpack (rm ensures no stale files from a previous version)
    rm -rf "$HOME/.local/prism$suffix.app"
    tar -xzf "$temp/prism-linux-$arch.tar.gz" -C "$HOME/.local/"

    # Setup ~/.local directories
    mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications"

    # Link the binary
    if [ -f "$HOME/.local/prism$suffix.app/bin/prism" ]; then
        ln -sf "$HOME/.local/prism$suffix.app/bin/prism" "$HOME/.local/bin/prism"
    else
        # support for versions before 0.139.x.
        ln -sf "$HOME/.local/prism$suffix.app/bin/cli" "$HOME/.local/bin/prism"
    fi

    # Copy .desktop file
    desktop_file_path="$HOME/.local/share/applications/${appid}.desktop"
    src_dir="$HOME/.local/prism$suffix.app/share/applications"
    if [ -f "$src_dir/${appid}.desktop" ]; then
        cp "$src_dir/${appid}.desktop" "${desktop_file_path}"
    else
        # Fallback for older tarballs
        cp "$src_dir/prism$suffix.desktop" "${desktop_file_path}"
    fi
    sed -i \
        -e "s|Icon=prism|Icon=$HOME/.local/prism$suffix.app/share/icons/hicolor/512x512/apps/prism.png|g" \
        -e "s|Exec=prism|Exec=$HOME/.local/prism$suffix.app/bin/prism|g" \
        "${desktop_file_path}"
}

macos() {
    echo "Downloading Prism version: $PRISM_VERSION"
    curl "https://cloud.zed.dev/releases/$channel/$PRISM_VERSION/download?asset=zed&os=macos&arch=$arch&source=install.sh" > "$temp/Prism-$arch.dmg"
    hdiutil attach -quiet "$temp/Prism-$arch.dmg" -mountpoint "$temp/mount"
    app="$(cd "$temp/mount/"; echo *.app)"
    echo "Installing $app"
    if [ -d "/Applications/$app" ]; then
        echo "Removing existing $app"
        rm -rf "/Applications/$app"
    fi
    ditto "$temp/mount/$app" "/Applications/$app"
    hdiutil detach -quiet "$temp/mount"

    mkdir -p "$HOME/.local/bin"
    # Link the binary
    ln -sf "/Applications/$app/Contents/MacOS/cli" "$HOME/.local/bin/prism"
}

main "$@"
