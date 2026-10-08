#!/bin/sh
# Kiro TTY container entrypoint.
#
# Sets the kiro user's password, propagates the Kiro API key into login
# sessions, starts the SSH daemon in the background, then runs a telnet daemon
# in the foreground as the container's main process. Both login paths use the
# kiro user, whose login shell is Kiro TTY.

set -eu

PW="${KIRO_TTY_PASSWORD:-kiro}"
echo "kiro:${PW}" | chpasswd

if [ "$PW" = "kiro" ]; then
    echo "############################################################"
    echo "# WARNING: 'kiro' user is using the DEFAULT password 'kiro'."
    echo "# Set KIRO_TTY_PASSWORD to change it.                        "
    echo "#                                                           "
    echo "# telnet is PLAINTEXT and the Kiro agent runs with          "
    echo "# --trust-all-tools inside this container. Do NOT expose    "
    echo "# these ports to an untrusted network. Keep them bound to   "
    echo "# 127.0.0.1 unless you fully understand the risk.           "
    echo "############################################################"
fi

# telnet's /bin/login and sshd both start login sessions with a CLEAN
# environment, so container-level env vars (like KIRO_API_KEY) do not reach the
# Kiro TTY login shell on their own. Persist the ones we care about to a file
# that kiro-shell sources on each login.
{
    [ -n "${KIRO_API_KEY:-}" ]     && printf 'export KIRO_API_KEY=%s\n' "$KIRO_API_KEY"
    [ -n "${KIRO_TTY_WIDTH:-}" ]   && printf 'export KIRO_TTY_WIDTH=%s\n' "$KIRO_TTY_WIDTH"
    [ -n "${KIRO_TTY_NEWLINE:-}" ] && printf 'export KIRO_TTY_NEWLINE=%s\n' "$KIRO_TTY_NEWLINE"
    true
} > /etc/kiro-tty.env
chown root:kiro /etc/kiro-tty.env
chmod 640 /etc/kiro-tty.env

# Kiro authenticates via KIRO_API_KEY (headless mode). Warn clearly if it is
# missing, since Kiro will then have no credentials.
if [ -z "${KIRO_API_KEY:-}" ]; then
    echo "WARNING: KIRO_API_KEY is not set. Kiro has no credentials and every"
    echo "         prompt will fail. Put KIRO_API_KEY=ksk_... in a .env file"
    echo "         next to docker-compose.yml (see that file for details)."
fi

# Generate SSH host keys at runtime (never baked into the image).
ssh-keygen -A >/dev/null 2>&1 || true

# SSH daemon in the background.
/usr/sbin/sshd

# Raw TCP listener (no telnet protocol) for retro terminals behind WiFi modems.
# busybox telnetd opens every connection with IAC option negotiation
# (ff fd 01 ff fd 1f ff fb 01 ff fb 03). A modem that does not strip those
# hands them to the terminal, which prints them as ^A ^_ ^C garbage, and some
# terminal programs choke on them. This port speaks plain bytes over a pty,
# like a classic BBS. It uses the C64 profile (40 cols, ^H erase); `login -p`
# preserves KIRO_TTY_PROFILE into the login shell. c64-login draws the ANSI
# Kiro ghost banner first, then execs /bin/login -p.
KIRO_TTY_PROFILE=c64 socat \
    TCP-LISTEN:6400,reuseaddr,fork \
    EXEC:/usr/local/bin/c64-login,pty,setsid,ctty,stderr,sane &

echo "kiro-tty: SSH on :22, telnet on :23, raw C64 profile on :6400. Login shell is Kiro TTY."
echo "kiro-tty: agent=tty  auth=KIRO_API_KEY"

# telnetd in the foreground -> becomes the container's long-running process.
# busybox telnetd spawns /bin/login for each connection, which authenticates
# the kiro user and launches its login shell (Kiro TTY).
exec busybox telnetd -F -l /bin/login -p 23
