# syntax=docker/dockerfile:1

# ---------------------------------------------------------------------------
# Build stage: compile the kiro-tty binary.
# rust:1-bookworm has glibc 2.36; the resulting binary runs on the Ubuntu 24.04
# runtime (glibc 2.39). We statically verified the mounted kiro-cli needs only
# GLIBC_2.34, so everything is compatible on this base.
# ---------------------------------------------------------------------------
FROM rust:1-bookworm AS builder
WORKDIR /src
# Copy manifests first for layer caching, then sources.
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release && strip target/release/kiro-tty

# ---------------------------------------------------------------------------
# Runtime stage: Ubuntu 24.04 with SSH + telnet, where the kiro user's login
# shell is Kiro TTY. kiro-cli itself and the Kiro credentials are NOT baked in;
# they are mounted at runtime via docker-compose (see docker-compose.yml).
# ---------------------------------------------------------------------------
FROM ubuntu:24.04

# openssh-server: SSH access.
# busybox-static: provides telnetd.
# login: /bin/login for the telnet path.
# ca-certificates: TLS trust for Kiro/AWS API calls.
# libgcc/libstdc++: shared libs the mounted kiro-cli and bun may need.
RUN apt-get update && apt-get install -y --no-install-recommends \
        openssh-server \
        busybox-static \
        login \
        ca-certificates \
        libgcc-s1 \
        libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /run/sshd

# Ubuntu 24.04 ships a default 'ubuntu' user at uid 1000. Remove it so the kiro
# user can take uid 1000, matching the host ownership of the mounted Kiro
# credential store (files are 0600 owned by uid 1000 on the host).
RUN userdel -r ubuntu 2>/dev/null || true \
    && useradd -m -u 1000 -s /usr/local/bin/kiro-shell kiro \
    && mkdir -p /home/kiro/workspace \
                /home/kiro/.kiro/agents \
                /home/kiro/.local/share/kiro-cli \
    && chown -R kiro:kiro /home/kiro

# Install the Kiro CLI from the official Linux zip (glibc x86_64 build). This
# bakes the three binaries (kiro-cli launcher + kiro-cli-chat + kiro-cli-term)
# into the image, so no host binaries need to be mounted. curl/unzip are
# build-only and purged afterward to keep the layer lean.
ARG KIRO_CLI_URL=https://desktop-release.q.us-east-1.amazonaws.com/latest/kirocli-x86_64-linux.zip
RUN apt-get update && apt-get install -y --no-install-recommends curl unzip \
    && curl --proto '=https' --tlsv1.2 -sSf "$KIRO_CLI_URL" -o /tmp/kirocli.zip \
    && unzip -q /tmp/kirocli.zip -d /tmp \
    && cp /tmp/kirocli/bin/kiro-cli /tmp/kirocli/bin/kiro-cli-chat /tmp/kirocli/bin/kiro-cli-term \
          /usr/local/bin/ \
    && chmod +x /usr/local/bin/kiro-cli /usr/local/bin/kiro-cli-chat /usr/local/bin/kiro-cli-term \
    && rm -rf /tmp/kirocli /tmp/kirocli.zip \
    && apt-get purge -y curl unzip && apt-get autoremove -y \
    && rm -rf /var/lib/apt/lists/*

# The kiro-tty binary.
COPY --from=builder /src/target/release/kiro-tty /usr/local/bin/kiro-tty

# The self-contained tty agent (resources:[] so it needs no rules files).
COPY docker/tty.json /home/kiro/.kiro/agents/tty.json

# Login shell + entrypoint.
COPY docker/kiro-shell /usr/local/bin/kiro-shell
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh

# Custom pre-login banner. telnet/ssh are network logins, so login shows
# /etc/issue.net; /etc/issue is set too for completeness.
COPY docker/issue /etc/issue
COPY docker/issue /etc/issue.net

# Allow SSH password auth for the kiro user; forbid root login; keep logins
# quiet (no MOTD / last-login banners); show the custom banner pre-auth.
RUN printf 'PasswordAuthentication yes\nPermitRootLogin no\nPrintMotd no\nPrintLastLog no\nBanner /etc/issue\n' \
        > /etc/ssh/sshd_config.d/kiro-tty.conf \
    && chmod +x /usr/local/bin/kiro-shell /usr/local/bin/entrypoint.sh \
    && echo /usr/local/bin/kiro-shell >> /etc/shells \
    && chown -R kiro:kiro /home/kiro/.kiro \
    # Silence Ubuntu login banners for both telnet (/bin/login) and SSH:
    # remove the MOTD sources, disable pam_motd, and hush per-user login noise.
    && rm -rf /etc/update-motd.d/* /etc/motd.d /etc/motd /etc/legal \
    && sed -i '/pam_motd/d' /etc/pam.d/login /etc/pam.d/sshd \
    && touch /home/kiro/.hushlogin \
    && chown kiro:kiro /home/kiro/.hushlogin

EXPOSE 22 23
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
