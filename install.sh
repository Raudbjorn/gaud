#!/bin/bash

# Gaud - Multi-user LLM Proxy Installation Script
# Builds, installs, and configures gaud as a systemd service.
#
# Usage:
#   sudo ./install.sh             # Install gaud
#   sudo ./install.sh --uninstall # Remove gaud

set -eo pipefail

# Color codes for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
PURPLE='\033[0;35m'
CYAN='\033[0;36m'
NC='\033[0m'

ROCKET="🚀"
CHECK="✅"
CROSS="❌"
WARNING="⚠️"
GEAR="⚙️"

# Installation paths
BINARY_SRC="target/release/gaud"
BINARY_DST="/usr/local/bin/gaud"
CONFIG_SRC="llm-proxy.toml"
CONFIG_DIR="/etc/gaud"
CONFIG_DST="$CONFIG_DIR/llm-proxy.toml"
DATA_DIR="/var/lib/gaud"
TOKEN_DIR="$DATA_DIR/tokens"
SERVICE_SRC="gaud.service"
SERVICE_DST="/etc/systemd/system/gaud.service"
SERVICE_USER="gaud"
SERVICE_GROUP="gaud"

# Script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

print_header() {
    echo -e "\n${PURPLE}╔═══════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${PURPLE}║          Gaud - Multi-user LLM Proxy Installer                ║${NC}"
    echo -e "${PURPLE}╚═══════════════════════════════════════════════════════════════╝${NC}"
}

print_success() {
    echo -e "${GREEN}${CHECK} $1${NC}"
}

print_error() {
    echo -e "${RED}${CROSS} $1${NC}" >&2
}

print_warning() {
    echo -e "${YELLOW}${WARNING} $1${NC}"
}

print_info() {
    echo -e "${BLUE}${GEAR} $1${NC}"
}

print_step() {
    echo -e "\n${CYAN}${GEAR} [$1/$TOTAL_STEPS] $2${NC}"
    echo -e "${CYAN}$(printf '%.0s─' {1..60})${NC}"
}

check_root() {
    if [ "$(id -u)" -ne 0 ]; then
        print_error "This script must be run as root (use sudo)"
        exit 1
    fi
}

# ─── INSTALL ────────────────────────────────────────────────────────────────

TOTAL_STEPS=8

do_install() {
    print_header
    echo ""
    print_info "Installing gaud..."

    # Step 1: Check prerequisites
    print_step 1 "Checking prerequisites"

    if ! command -v cargo >/dev/null 2>&1; then
        print_error "cargo not found. Install Rust: https://rustup.rs/"
        exit 1
    fi
    print_success "cargo: $(cargo --version)"

    if ! command -v rustc >/dev/null 2>&1; then
        print_error "rustc not found. Install Rust: https://rustup.rs/"
        exit 1
    fi
    print_success "rustc: $(rustc --version)"

    # Step 2: Build release binary
    print_step 2 "Building release binary"

    local binary_path="$SCRIPT_DIR/$BINARY_SRC"

    # Build as the invoking user (not root) if SUDO_USER is set
    if [ -n "$SUDO_USER" ]; then
        print_info "Building as user '$SUDO_USER'..."
        sudo -u "$SUDO_USER" bash -c "cd '$SCRIPT_DIR' && cargo build --release"
    else
        cd "$SCRIPT_DIR"
        cargo build --release
    fi

    if [ ! -f "$binary_path" ]; then
        print_error "Build failed: binary not found at $binary_path"
        exit 1
    fi

    local binary_size
    binary_size=$(du -h "$binary_path" | cut -f1)
    print_success "Binary built: $binary_path ($binary_size)"

    # Step 3: Create system user/group
    print_step 3 "Creating system user and group"

    if getent group "$SERVICE_GROUP" >/dev/null 2>&1; then
        print_success "Group '$SERVICE_GROUP' already exists"
    else
        groupadd --system "$SERVICE_GROUP"
        print_success "Created group '$SERVICE_GROUP'"
    fi

    if getent passwd "$SERVICE_USER" >/dev/null 2>&1; then
        print_success "User '$SERVICE_USER' already exists"
    else
        useradd --system \
            --gid "$SERVICE_GROUP" \
            --home-dir "$DATA_DIR" \
            --shell /usr/sbin/nologin \
            --comment "Gaud LLM Proxy" \
            "$SERVICE_USER"
        print_success "Created user '$SERVICE_USER'"
    fi

    # Step 4: Install binary
    print_step 4 "Installing binary"

    install -m 0755 "$binary_path" "$BINARY_DST"
    print_success "Installed binary: $BINARY_DST"

    # Step 5: Create directories
    print_step 5 "Creating directories"

    install -d -m 0755 -o "$SERVICE_USER" -g "$SERVICE_GROUP" "$CONFIG_DIR"
    print_success "Config directory: $CONFIG_DIR"

    install -d -m 0750 -o "$SERVICE_USER" -g "$SERVICE_GROUP" "$DATA_DIR"
    print_success "Data directory: $DATA_DIR"

    install -d -m 0750 -o "$SERVICE_USER" -g "$SERVICE_GROUP" "$TOKEN_DIR"
    print_success "Token directory: $TOKEN_DIR"

    # Step 6: Install default config
    print_step 6 "Installing configuration"

    local config_src_path="$SCRIPT_DIR/$CONFIG_SRC"
    if [ -f "$CONFIG_DST" ]; then
        print_warning "Config already exists at $CONFIG_DST - not overwriting"
        print_info "New default config available at: $config_src_path"

        # Check if configs differ
        if [ -f "$config_src_path" ] && ! diff -q "$config_src_path" "$CONFIG_DST" >/dev/null 2>&1; then
            print_warning "Your config differs from the default. Review changes manually."
        fi
    else
        if [ -f "$config_src_path" ]; then
            install -m 0640 -o "$SERVICE_USER" -g "$SERVICE_GROUP" "$config_src_path" "$CONFIG_DST"
            print_success "Installed default config: $CONFIG_DST"
        else
            print_warning "No default config found at $config_src_path"
            print_info "You'll need to create $CONFIG_DST manually"
        fi
    fi

    # Step 7: Install systemd unit
    print_step 7 "Installing systemd service"

    local service_src_path="$SCRIPT_DIR/$SERVICE_SRC"
    if [ ! -f "$service_src_path" ]; then
        print_error "Service file not found: $service_src_path"
        exit 1
    fi

    install -m 0644 "$service_src_path" "$SERVICE_DST"
    print_success "Installed service: $SERVICE_DST"

    systemctl daemon-reload
    print_success "Systemd daemon reloaded"

    systemctl enable gaud.service
    print_success "Service enabled (will start on boot)"

    # Step 8: Post-install summary
    print_step 8 "Installation complete"

    echo ""
    echo -e "${GREEN}${ROCKET} Gaud has been installed successfully!${NC}"
    echo ""
    echo -e "${CYAN}Next steps:${NC}"
    echo ""
    echo -e "  1. Edit the configuration file:"
    echo -e "     ${CYAN}sudo nano $CONFIG_DST${NC}"
    echo ""
    echo -e "  2. Start the service:"
    echo -e "     ${CYAN}sudo systemctl start gaud${NC}"
    echo ""
    echo -e "  3. Check status:"
    echo -e "     ${CYAN}sudo systemctl status gaud${NC}"
    echo ""
    echo -e "  4. View logs:"
    echo -e "     ${CYAN}journalctl -u gaud -f${NC}"
    echo ""
    echo -e "${YELLOW}Service details:${NC}"
    echo -e "  Binary:  $BINARY_DST"
    echo -e "  Config:  $CONFIG_DST"
    echo -e "  Data:    $DATA_DIR"
    echo -e "  Tokens:  $TOKEN_DIR"
    echo -e "  Service: $SERVICE_DST"
    echo -e "  User:    $SERVICE_USER"
    echo -e "  Port:    8400 (default)"
}

# ─── UNINSTALL ──────────────────────────────────────────────────────────────

do_uninstall() {
    print_header
    echo ""
    print_warning "Uninstalling gaud..."
    echo ""

    # Stop and disable service
    if systemctl is-active --quiet gaud.service 2>/dev/null; then
        print_info "Stopping gaud service..."
        systemctl stop gaud.service
        print_success "Service stopped"
    fi

    if systemctl is-enabled --quiet gaud.service 2>/dev/null; then
        print_info "Disabling gaud service..."
        systemctl disable gaud.service
        print_success "Service disabled"
    fi

    # Remove service file
    if [ -f "$SERVICE_DST" ]; then
        rm -f "$SERVICE_DST"
        systemctl daemon-reload
        print_success "Removed service file: $SERVICE_DST"
    fi

    # Remove binary
    if [ -f "$BINARY_DST" ]; then
        rm -f "$BINARY_DST"
        print_success "Removed binary: $BINARY_DST"
    fi

    # Ask about data and config
    echo ""
    print_warning "The following directories were NOT removed (may contain user data):"
    echo -e "  ${YELLOW}$CONFIG_DIR${NC} (configuration)"
    echo -e "  ${YELLOW}$DATA_DIR${NC} (database, tokens)"
    echo ""

    read -rp "Remove configuration directory ($CONFIG_DIR)? [y/N] " response
    if [[ "$response" =~ ^[Yy]$ ]]; then
        rm -rf "$CONFIG_DIR"
        print_success "Removed: $CONFIG_DIR"
    fi

    read -rp "Remove data directory ($DATA_DIR)? [y/N] " response
    if [[ "$response" =~ ^[Yy]$ ]]; then
        rm -rf "$DATA_DIR"
        print_success "Removed: $DATA_DIR"
    fi

    # Ask about user/group
    if getent passwd "$SERVICE_USER" >/dev/null 2>&1; then
        read -rp "Remove system user '$SERVICE_USER'? [y/N] " response
        if [[ "$response" =~ ^[Yy]$ ]]; then
            userdel "$SERVICE_USER" 2>/dev/null || true
            print_success "Removed user: $SERVICE_USER"
        fi
    fi

    if getent group "$SERVICE_GROUP" >/dev/null 2>&1; then
        read -rp "Remove system group '$SERVICE_GROUP'? [y/N] " response
        if [[ "$response" =~ ^[Yy]$ ]]; then
            groupdel "$SERVICE_GROUP" 2>/dev/null || true
            print_success "Removed group: $SERVICE_GROUP"
        fi
    fi

    echo ""
    print_success "Gaud has been uninstalled"
}

# ─── MAIN ───────────────────────────────────────────────────────────────────

check_root

case "${1:-}" in
    --uninstall|-u)
        do_uninstall
        ;;
    --help|-h)
        echo "Usage: sudo $0 [--uninstall]"
        echo ""
        echo "Options:"
        echo "  (none)       Install gaud as a systemd service"
        echo "  --uninstall  Remove gaud installation"
        echo "  --help       Show this help message"
        ;;
    "")
        do_install
        ;;
    *)
        print_error "Unknown option: $1"
        echo "Usage: sudo $0 [--uninstall]"
        exit 1
        ;;
esac
