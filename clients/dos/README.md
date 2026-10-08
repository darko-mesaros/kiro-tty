# DOS clients

Files for dialing Kiro TTY from an IBM PC running MS-DOS through a serial
WiFi modem. Copy them to the PC (for example into `C:\KERMIT`).

| File | What it is |
|------|------------|
| `KIROTTY.INI` / `KIROTTY.BAT` | MS-DOS Kermit script that dials port 6401 at 300 baud on COM1. Edit the `define` lines for your port, speed and host. |
| `term2.asm` | TERM2, a tiny polled terminal (125 bytes) for 8250 UARTs whose "data ready" status bit is broken. |
| `TERM2.COM` | Prebuilt TERM2. |

## Why TERM2 exists

On the 5160 this was built for, the IBM Asynchronous Communications Adapter's
8250 receives bytes but never sets LSR bit 0 (data ready). Every normal serial
program waits on that bit, so Kermit, the BIOS (`SET PORT BIOS1`) and a plain
polled terminal all sent fine but never showed anything received. The chip's
interrupt-identification register (IIR) still drops its "nothing pending" bit
when a byte arrives, so TERM2 polls that instead. It never raises a real
interrupt (OUT2 stays off).

TERM2 is hardcoded to COM1, 300 baud, 8N1. Ctrl-] quits. On a healthy UART,
use Kermit instead; the real fix for a broken 8250 is a drop-in 16550AN.

## Building TERM2

```bash
nasm -f bin -Werror -o TERM2.COM term2.asm
```

To type it in on a PC with no way to copy files, print DEBUG `e` lines:

```bash
od -An -v -tx1 TERM2.COM
```

and enter them with `DEBUG`, then `n TERM2.COM`, `r cx` (125 = `7D`), `w`.
