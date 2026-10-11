# DOS clients

Files for dialing Kiro TTY from an IBM PC running MS-DOS through a serial
WiFi modem. Copy them to the PC (for example into `C:\KERMIT`).

| File | What it is |
|------|------------|
| `KIROTTY.INI` / `KIROTTY.BAT` | MS-DOS Kermit 3.14 script that dials port 6401 at 300 baud on COM1, no flow control. Edit the `define` lines for your port, speed and host. |
| `term2.asm` | TERM2, a tiny polled terminal (125 bytes). Fallback only, see below. |
| `TERM2.COM` | Prebuilt TERM2. |

Tested on an IBM 5160 with an IBM Asynchronous Communications Adapter and a
WiModem232 at 300 baud.

## Speed

The WiModem232 ships at 300 baud. To go faster, set the modem first, then
Kermit: `AT*B1200` and `AT&W` on the modem, then change `define _ktspeed` to
match. Use `AT*T0` (no telnet translation, the default) for the raw ports
6400/6401, and `AT*T1` only for the telnet port 2323.

## Colours

On a CGA, EGA or VGA card, uncomment the `set terminal color` line in
`KIROTTY.INI`. The numbers are `1` (bright), `3x` foreground, `4x`
background, `20` (keep the colours), where x is red 1 + green 2 + blue 4.

## When the PC can send but never receive

If Kermit sends (the modem sees your typing) but nothing ever comes back,
check for a second card answering at the same I/O address before blaming the
UART. On the 5160 this was built on, a Quadram Quadboard multifunction card
also had its serial port at COM1 (3F8). On ISA that crashes nothing: writes
reach both chips, and reads come back as the two chips' values ANDed
together. The idle chip's LSR (`60`) masked the real one (`61`), so the
"data ready" bit never appeared to set, and IIR read a garbled `00`. It looks
exactly like a broken 8250. Pull every card you do not need, or move the
other port to COM2.

Quick check with DEBUG and a paperclip across pins 2 and 3 of the DB25:

```
o 3fb 83
o 3f8 80
o 3f9 01
o 3fb 03
i 3f8
o 3f8 41
i 3fd        61 = healthy, 60 = something is masking the flag
i 3f8        41
```

## TERM2

TERM2 polls the IIR's "nothing pending" bit instead of the LSR, which happens
to work even while a second chip masks the LSR. It was the workaround before
the real cause was found and is kept as a fallback. Hardcoded to COM1, 300
baud, 8N1. Ctrl-] quits. It never raises a real interrupt (OUT2 stays off).

Build:

```bash
nasm -f bin -Werror -o TERM2.COM term2.asm
```

To type it in on a PC with no way to copy files, print DEBUG `e` lines:

```bash
od -An -v -tx1 TERM2.COM
```

and enter them with `DEBUG`, then `n TERM2.COM`, `r cx` (125 = `7D`), `w`.
