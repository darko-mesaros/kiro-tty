; TERM2.COM - polled serial terminal for an 8250 whose LSR "data ready" bit
; is broken (IBM 5160, IBM Async Card, diagnosed 2026-10-08).
;
; On this chip LSR bit 0 never sets, but the interrupt-identification
; register (IIR, 3FA) still drops its "no interrupt pending" bit (bit 0)
; when a byte arrives. So: enable only the received-data interrupt in IER
; (the IRQ line itself is never used; the PIC is left alone), poll IIR bit 0,
; and read RBR when it goes low. Reading RBR clears the condition. MSR and LSR
; are read each time too, so a modem-status or line-status event can never
; leave IIR stuck "pending".
;
; Keys go out the port; received bytes go to the screen via BIOS teletype.
; Ctrl-] quits.
;
; Build: nasm -f bin -o TERM2.COM term2.asm

        cpu     8086
        org     100h

COM     equ     3F8h            ; COM1 base
RBR     equ     COM+0           ; receive buffer / transmit holding (DLAB=0)
DLL     equ     COM+0           ; divisor latch low (DLAB=1)
IER     equ     COM+1           ; interrupt enable
IIR     equ     COM+2           ; interrupt identification
LCR     equ     COM+3           ; line control
MCR     equ     COM+4           ; modem control
LSR     equ     COM+5           ; line status
MSR     equ     COM+6           ; modem status

DIVISOR equ     115200 / 300    ; 300 baud
QUITKEY equ     1Dh             ; Ctrl-]

start:
        mov     dx, LCR         ; DLAB on, so COM+0/+1 are the divisor
        mov     al, 83h
        out     dx, al
        mov     dx, DLL
        mov     al, DIVISOR & 0FFh
        out     dx, al
        inc     dx              ; DLM
        mov     al, DIVISOR >> 8
        out     dx, al
        mov     dx, LCR         ; 8 data bits, no parity, 1 stop, DLAB off
        mov     al, 03h
        out     dx, al
        mov     dx, MCR         ; DTR + RTS. OUT2 stays 0, so the card never
        mov     al, 03h         ; drives the IRQ line to the PIC.
        out     dx, al
        mov     dx, RBR         ; flush any stale byte and status
        in      al, dx
        mov     dx, LSR
        in      al, dx
        mov     dx, MSR
        in      al, dx
        mov     dx, IER         ; enable the received-data interrupt only
        mov     al, 01h
        out     dx, al

poll:
        mov     dx, IIR         ; bit 0 low = something pending (a byte)
        in      al, dx
        test    al, 01h
        jnz     kbd
        mov     dx, RBR         ; read the byte (clears the condition)
        in      al, dx
        mov     cl, al
        mov     dx, MSR         ; clear any other pending sources
        in      al, dx
        mov     dx, LSR
        in      al, dx
        mov     al, cl
        mov     ah, 0Eh         ; BIOS teletype (handles CR, LF, BS, BEL)
        xor     bx, bx
        int     10h
        jmp     poll

kbd:
        mov     ah, 01h         ; key waiting?
        int     16h
        jz      poll
        xor     ah, ah          ; read it
        int     16h
        cmp     al, QUITKEY
        je      quit
        or      al, al          ; extended key (arrows etc.): ignore
        jz      poll
        mov     ah, al          ; save the key
        mov     dx, LSR         ; wait for the transmit register to empty
.wait:
        in      al, dx
        test    al, 20h
        jz      .wait
        mov     dx, RBR
        mov     al, ah
        out     dx, al
        jmp     poll

quit:
        mov     dx, IER         ; leave the UART quiet
        xor     al, al
        out     dx, al
        mov     ax, 4C00h
        int     21h
