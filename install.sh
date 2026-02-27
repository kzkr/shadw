#!/bin/sh
set -eu

REPO="kzkr/shadw"

main() {
    os=$(uname -s)
    arch=$(uname -m)

    case "$os" in
        Darwin) os_part="apple-darwin" ;;
        Linux)  os_part="unknown-linux-gnu" ;;
        *)
            echo "Error: unsupported OS: $os" >&2
            exit 1
            ;;
    esac

    case "$arch" in
        arm64|aarch64) arch_part="aarch64" ;;
        x86_64)        arch_part="x86_64" ;;
        *)
            echo "Error: unsupported architecture: $arch" >&2
            exit 1
            ;;
    esac

    target="${arch_part}-${os_part}"

    echo "Detected platform: ${target}"

    # Get latest release tag
    tag=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
        | grep '"tag_name"' | head -1 | cut -d'"' -f4)

    if [ -z "$tag" ]; then
        echo "Error: could not determine latest release" >&2
        exit 1
    fi

    echo "Latest version: ${tag}"

    archive="shadw-${tag}-${target}.tar.gz"
    url="https://github.com/${REPO}/releases/download/${tag}/${archive}"

    tmpdir=$(mktemp -d)
    trap 'rm -rf "$tmpdir"' EXIT

    echo "Downloading ${archive}..."
    curl -fsSL "$url" -o "${tmpdir}/${archive}"

    tar xzf "${tmpdir}/${archive}" -C "$tmpdir"

    # Install binary
    if [ -w /usr/local/bin ]; then
        install_dir="/usr/local/bin"
    else
        install_dir="${HOME}/.local/bin"
        mkdir -p "$install_dir"
    fi

    mv "${tmpdir}/shadw" "${install_dir}/shadw"
    chmod +x "${install_dir}/shadw"

    echo "Installed shadw to ${install_dir}/shadw"

    # Verify
    if "${install_dir}/shadw" --version >/dev/null 2>&1; then
        echo "$(${install_dir}/shadw --version)"
    fi

    # Warn if install dir not in PATH
    case ":${PATH}:" in
        *":${install_dir}:"*) ;;
        *)
            echo ""
            echo "Warning: ${install_dir} is not in your PATH."
            echo "Add it with:  export PATH=\"${install_dir}:\$PATH\""
            ;;
    esac

    echo ""
    echo "Run 'shadw --help' to get started."
}

main
