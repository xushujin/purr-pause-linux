#!/bin/sh
# Post-install: configure Electron sandbox and enable autostart for the
# desktop user who installed the deb with sudo.

APP_DIR="${PURR_PAUSE_INSTALL_DIR:-/opt/purr-pause}"
APP_EXEC="$APP_DIR/purr-pause"
CHROME_SANDBOX="$APP_DIR/chrome-sandbox"
TARGET_USER=""
TARGET_HOME=""
TARGET_GROUP=""

configure_chrome_sandbox() {
  if [ ! -e "$CHROME_SANDBOX" ]; then
    echo "胖猫暂停一下（PurrPause） chrome-sandbox not found at $CHROME_SANDBOX; skipping sandbox permission setup."
    return 0
  fi

  if ! chown root:root "$CHROME_SANDBOX"; then
    echo "胖猫暂停一下（PurrPause） failed to set chrome-sandbox owner: $CHROME_SANDBOX" >&2
    return 1
  fi

  if ! chmod 4755 "$CHROME_SANDBOX"; then
    echo "胖猫暂停一下（PurrPause） failed to set chrome-sandbox mode 4755: $CHROME_SANDBOX" >&2
    return 1
  fi

  echo "胖猫暂停一下（PurrPause） chrome-sandbox configured: $CHROME_SANDBOX"
}

configure_chrome_sandbox || exit 1

if [ -n "$SUDO_USER" ] && [ "$SUDO_USER" != "root" ]; then
  TARGET_USER="$SUDO_USER"
elif [ -n "$PKEXEC_UID" ]; then
  TARGET_USER="$(getent passwd "$PKEXEC_UID" | cut -d: -f1)"
fi

if [ -n "$TARGET_USER" ]; then
  TARGET_HOME="$(getent passwd "$TARGET_USER" | cut -d: -f6)"
  TARGET_GROUP="$(getent passwd "$TARGET_USER" | cut -d: -f4)"
fi

if [ -z "$TARGET_HOME" ] || [ ! -d "$TARGET_HOME" ]; then
  echo "胖猫暂停一下（PurrPause） autostart not created: target desktop user could not be detected."
  exit 0
fi

AUTOSTART_DIR="$TARGET_HOME/.config/autostart"
AUTOSTART_FILE="$AUTOSTART_DIR/purr-pause.desktop"

mkdir -p "$AUTOSTART_DIR"

cat > "$AUTOSTART_FILE" << EOF
[Desktop Entry]
Type=Application
Name=胖猫暂停一下（PurrPause）
Comment=胖猫暂停一下 - 屏幕休息提醒
Exec=$APP_EXEC
Icon=purr-pause
Terminal=false
Hidden=false
NoDisplay=false
X-GNOME-Autostart-enabled=true
Categories=Utility;
EOF

chown "$TARGET_USER:$TARGET_GROUP" "$TARGET_HOME/.config" "$AUTOSTART_DIR" "$AUTOSTART_FILE" 2>/dev/null || true
chmod 644 "$AUTOSTART_FILE"

echo "胖猫暂停一下（PurrPause） autostart entry created for $TARGET_USER."
