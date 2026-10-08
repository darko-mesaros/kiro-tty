# Kiro TTY

It's Kiro for dumb terminals.

Kiro TTY is a small, line-oriented frontend that lets anything that can send
text over a wire talk to Kiro: a DECwriter, a Commodore 64, a teletype, or a
plain telnet client. It runs `kiro-cli acp` as a local subprocess, speaks the
Agent Client Protocol over stdio, and does all rendering itself. The terminal
only ever receives plain 7-bit ASCII, word-wrapped to its width. No ANSI
escapes, cursor addressing, or Unicode.

## How to run?

If you have `docker compose`, and `just`... Just run this:

```bash
git clone <repo> && cd kiro-tty
echo 'KIRO_API_KEY=ksk_...' > .env
just up
telnet localhost 2323     # kiro / kiro
```

The image works on both x86_64 and aarch64 hosts (Graviton, Raspberry Pi,
Apple Silicon). The matching Kiro CLI build is picked automatically.

## Ports

| Port | Protocol | Profile | Use it for |
|------|----------|---------|------------|
| 2323 | telnet | 80 columns | telnet clients, printing terminals (DECwriter) |
| 2222 | SSH | 80 columns | modern clients |
| 6400 | raw TCP | 40 columns, `^H` erase | Commodore 64 with NovaTerm, WiFi modems that do not speak telnet |
| 6401 | raw TCP | 80 columns | IBM PC with a DOS terminal program (MS-DOS Kermit, TERM2) |

Ports 6400 and 6401 send no telnet protocol bytes at all. The telnet port opens every
connection with option negotiation (`IAC DO ECHO`, `IAC WILL SGA`, ...). Some
WiFi modems pass those through raw, and terminal programs like NovaTerm print
them as `^A ^_ ^C` garbage and may drop the line. If you see junk at connect
time, use a raw port instead.

## Configuration

Everything is set in `.env` next to `docker-compose.yml` (it is gitignored).

| Variable | Default | Meaning |
|----------|---------|---------|
| `KIRO_API_KEY` | (required) | Kiro API key. The container acts as this identity and uses its credits. |
| `KIRO_TTY_PASSWORD` | `kiro` | Password for the `kiro` login user. Change it before exposing anything. |
| `KIRO_TTY_BIND` | `127.0.0.1` | Host address the ports are published on. |
| `KIRO_TTY_WIDTH` | `80` | Wrap width for the telnet and SSH ports. |
| `KIRO_TTY_NEWLINE` | `lf` | `lf`, `crlf`, or `cr`. Over telnet/SSH the pty already sends CR+LF, so `lf` is right. |
| `KIRO_TTY_C64_WIDTH` | `40` | Wrap width on port 6400. Try `39` if your terminal double-spaces full lines. |
| `KIRO_TTY_PC_WIDTH` | `80` | Wrap width on port 6401. |

## Vintage hardware

### DECwriter (or any printing terminal) over a serial-to-WiFi modem

Expose the container on your LAN, then dial the telnet port from the terminal:

```bash
# .env
KIRO_TTY_BIND=192.168.1.50      # this host's LAN IP
KIRO_TTY_PASSWORD=something-typeable
```

```
ATDT192.168.1.50:2323
```

Tested with a DECwriter IV at 300 baud. That is about 30 characters a second,
so a short answer prints in a few seconds. If long answers lose characters,
turn on XON/XOFF flow control on the modem. Ctrl-C cancels the current turn,
but text already buffered in the modem keeps printing until it drains.

### Commodore 64 with NovaTerm

Set NovaTerm to ANSI emulation in 40-column mode and dial the raw port:

```
ATDT192.168.1.50:6400
```

This profile wraps at 40 columns and treats the C64 DEL key (`^H`) as erase.
Kiro TTY also cleans every input line itself, so both `^H` and `^?` erase
correctly on any port, whichever one your terminal sends.

Before the login prompt, port 6400 clears the screen and prints a plain-text
Kiro ghost with block-letter `KIRO`, from `docker/c64-banner.txt`. It is
mounted into the container, so you can redraw it and see the change on your
next connection without rebuilding. Keep it to plain ASCII, 39 columns or
fewer, and avoid `\ | _ ~ { }`, which the C64 character set cannot show. (An
ANSI colour version was tried first; NovaTerm's ANSI mode ignored the
background colours on real hardware.) The banner is a fixed file, never model
output, so the "only plain text from Kiro" rule still holds.

### IBM PC with MS-DOS

Use a DOS terminal program on the PC's COM port, through the WiFi modem, and
dial the 80-column raw port:

```
ATDT192.168.1.50:6401
```

Ready-made files are in [`clients/dos/`](clients/dos/): a Kermit script
(`KIROTTY.INI` + `KIROTTY.BAT`) and TERM2, a 125-byte polled terminal for
machines whose 8250 serial chip is faulty. The PC profile keeps `^?` as erase
and echoes a `^H` raw, so the cursor steps back either way. Tested on an IBM
5160 (TERM2, 300 baud).

## Using it

Once logged in you are talking to Kiro. Type a request and press Return.

```
/help    show the commands
/new     start a fresh conversation
/cancel  stop the current response (Ctrl-C works too)
/quit    exit
```

Tool use shows up as short status lines, for example
`[TOOL] Running: uname -m`.

## Security

Read this before changing `KIRO_TTY_BIND`.

- Kiro runs with `--trust-all-tools`. Anyone who logs in can make it run any
  command inside the container. The container is the blast radius: it is not
  privileged and mounts no host paths.
- Telnet and port 6400 are plaintext, including the password.
- Usage is billed to the `KIRO_API_KEY` account.
- Docker-published ports bypass host firewalls like ufw.
- Bind to a specific LAN address, not `0.0.0.0`, so the ports are not also
  published on VPN or bridge interfaces.

Keep it on a network you trust.

## Building the binary directly

```bash
just release              # cargo build --release
just run                  # run locally against kiro-cli on your PATH
just test                 # cargo test
```

`kiro-tty --help` lists the options: `--width`, `--newline`, `--uppercase`
(for terminals without lowercase), `--agent`, `--model`, `--cwd`, `--log`.
