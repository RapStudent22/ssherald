#!/usr/bin/env bash
set -euo pipefail

# ============================================================================
# SSHerald: release-сборка, кросс-компиляция и установка зависимостей
#
# Использование:
#   ./build-release.sh deps           # установить все зависимости для сборки
#   ./build-release.sh all            # собрать все платформы (по умолчанию)
#   ./build-release.sh linux          # Linux x86_64
#   ./build-release.sh windows        # Windows x86_64 (MinGW)
#   ./build-release.sh macos          # macOS (требует host=macOS или osxcross)
#   ./build-release.sh current        # только текущая платформа
#
# SSH-бэкенд: russh + ring (чистый Rust, без OpenSSL / libssh2 / NASM / CMake)
# ============================================================================

PROJECT="ssherald"
OUT_DIR="dist"

# ── Цвета ──

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

info()  { echo -e "${CYAN}[INFO]${NC}  $*"; }
ok()    { echo -e "${GREEN}[OK]${NC}    $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC}  $*"; }
fail()  { echo -e "${RED}[FAIL]${NC}  $*"; }

# ── Определение дистрибутива ──

detect_distro() {
    if [ -f /etc/os-release ]; then
        . /etc/os-release
        case "$ID" in
            ubuntu|debian|linuxmint|pop)  echo "debian" ;;
            fedora|rhel|centos|rocky|alma) echo "fedora" ;;
            arch|manjaro|endeavouros)      echo "arch" ;;
            opensuse*|sles)                echo "suse" ;;
            *)                             echo "unknown" ;;
        esac
    elif command -v apt-get >/dev/null 2>&1; then
        echo "debian"
    elif command -v dnf >/dev/null 2>&1; then
        echo "fedora"
    elif command -v pacman >/dev/null 2>&1; then
        echo "arch"
    else
        echo "unknown"
    fi
}

# ── Установка зависимостей ──

install_deps() {
    info "Определение дистрибутива..."
    local distro
    distro=$(detect_distro)
    info "Дистрибутив: $distro"

    # 1. Системные пакеты
    info "Установка системных пакетов для сборки..."
    case "$distro" in
        debian)
            sudo apt-get update
            sudo apt-get install -y \
                build-essential \
                pkg-config \
                cmake \
                libxcb-render0-dev \
                libxcb-shape0-dev \
                libxcb-xfixes0-dev \
                libxkbcommon-dev \
                libgtk-3-dev \
                libwayland-dev \
                libfontconfig1-dev \
                libatk1.0-dev \
                libpango1.0-dev \
                libgdk-pixbuf-2.0-dev \
                mingw-w64
            ;;
        fedora)
            sudo dnf install -y \
                gcc gcc-c++ make \
                pkg-config cmake \
                libxcb-devel \
                libxkbcommon-devel \
                gtk3-devel \
                wayland-devel \
                fontconfig-devel \
                atk-devel \
                pango-devel \
                gdk-pixbuf2-devel \
                mingw64-gcc mingw64-gcc-c++
            ;;
        arch)
            sudo pacman -Syu --needed --noconfirm \
                base-devel \
                pkg-config cmake \
                libxcb \
                libxkbcommon \
                gtk3 \
                wayland \
                fontconfig \
                pango \
                gdk-pixbuf2 \
                mingw-w64-gcc
            ;;
        suse)
            sudo zypper install -y \
                gcc gcc-c++ make \
                pkg-config cmake \
                libxcb-devel \
                libxkbcommon-devel \
                gtk3-devel \
                wayland-devel \
                fontconfig-devel \
                pango-devel \
                gdk-pixbuf-devel \
                cross-x86_64-w64-mingw32-gcc
            ;;
        *)
            warn "Неизвестный дистрибутив. Установите вручную:"
            echo "  - build-essential / gcc / make"
            echo "  - pkg-config, cmake"
            echo "  - libxcb-render-dev, libxcb-shape-dev, libxcb-xfixes-dev"
            echo "  - libxkbcommon-dev, libgtk-3-dev, libwayland-dev"
            echo "  - libfontconfig-dev"
            echo "  - mingw-w64 (для кросс-компиляции под Windows)"
            return 1
            ;;
    esac
    ok "Системные пакеты установлены"

    # 2. Rust (через rustup)
    if command -v rustup >/dev/null 2>&1; then
        info "Rust уже установлен: $(rustc --version)"
        rustup update stable
    else
        info "Установка Rust через rustup..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        # shellcheck source=/dev/null
        source "$HOME/.cargo/env"
        ok "Rust установлен: $(rustc --version)"
    fi

    # 3. Rust targets для кросс-компиляции
    info "Установка Rust targets..."
    rustup target add x86_64-unknown-linux-gnu
    rustup target add x86_64-pc-windows-gnu
    ok "Rust targets установлены"

    echo
    ok "Все зависимости установлены. Можно собирать:"
    echo "  ./build-release.sh linux"
    echo "  ./build-release.sh windows"
    echo "  ./build-release.sh all"
}

# ── Вспомогательные функции сборки ──

ensure_target_installed() {
    local target="$1"
    if ! rustup target list --installed | grep -qx "$target"; then
        info "Устанавливаю Rust target: $target"
        rustup target add "$target"
    fi
}

check_mingw() {
    if ! command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
        fail "Не найден x86_64-w64-mingw32-gcc."
        echo "  Установите MinGW-w64: ./build-release.sh deps"
        return 1
    fi
}

# ── Сборка одного таргета ──

build_target() {
    local target="$1"
    echo
    info "=== Сборка: $target ==="

    ensure_target_installed "$target"

    # Проверки для кросс-компиляции
    if [[ "$target" == *"windows-gnu"* ]]; then
        check_mingw || return 1
    fi

    if [[ "$target" == *"apple-darwin"* ]] && [[ "$CURRENT_TARGET" != *"apple-darwin"* ]]; then
        warn "Пропуск $target: для кросс-сборки macOS нужен osxcross + Apple SDK"
        return 0
    fi

    # Сборка
    cargo build --release --target "$target"

    # Копирование бинарника
    local ext=""
    [[ "$target" == *windows* ]] && ext=".exe"

    local src="target/${target}/release/${PROJECT}${ext}"
    local dst="${OUT_DIR}/${PROJECT}-${VERSION}-${target}${ext}"

    if [ -f "$src" ]; then
        cp "$src" "$dst"
        local size
        size=$(du -h "$dst" | cut -f1)
        ok "$dst ($size)"
    else
        fail "Бинарник не найден: $src"
        return 1
    fi
}

# ── Точка входа ──

TARGETS_LINUX="x86_64-unknown-linux-gnu"
TARGETS_WINDOWS="x86_64-pc-windows-gnu"
TARGETS_MACOS="x86_64-apple-darwin aarch64-apple-darwin"

filter="${1:-all}"

# Обработка команды deps отдельно (до проверки rustc/cargo)
if [[ "$filter" == "deps" ]]; then
    install_deps
    exit 0
fi

# Для всех остальных команд нужен Rust
if ! command -v cargo >/dev/null 2>&1; then
    fail "cargo не найден. Сначала установите зависимости:"
    echo "  ./build-release.sh deps"
    exit 1
fi

VERSION=$(awk -F'"' '/^version = / {print $2; exit}' Cargo.toml)
CURRENT_TARGET=$(rustc -vV | awk '/^host:/ {print $2}')

mkdir -p "$OUT_DIR"

case "$filter" in
    linux)    targets="$TARGETS_LINUX" ;;
    windows)  targets="$TARGETS_WINDOWS" ;;
    macos)    targets="$TARGETS_MACOS" ;;
    current)  targets="$CURRENT_TARGET" ;;
    all)      targets="$TARGETS_LINUX $TARGETS_WINDOWS $TARGETS_MACOS" ;;
    *)
        echo "SSHerald build script"
        echo
        echo "Использование: $0 <команда>"
        echo
        echo "Команды:"
        echo "  deps      — установить все зависимости для сборки"
        echo "  all       — собрать все платформы (по умолчанию)"
        echo "  linux     — собрать Linux x86_64"
        echo "  windows   — собрать Windows x86_64 (MinGW)"
        echo "  macos     — собрать macOS (требует host=macOS)"
        echo "  current   — собрать только для текущей платформы"
        exit 1
        ;;
esac

echo "========================================"
echo " SSHerald v${VERSION} :: release build"
echo " SSH backend: russh + ring (pure Rust)"
echo "========================================"

failed=0

for t in $targets; do
    if ! build_target "$t"; then
        failed=$((failed + 1))
    fi
done

echo
echo "========================================"
if [ "$failed" -eq 0 ]; then
    ok "Все сборки завершены успешно"
else
    warn "Завершено с ошибками: $failed"
fi
echo "========================================"
echo
ls -lh "$OUT_DIR"/${PROJECT}-* 2>/dev/null || echo "(нет собранных файлов)"
