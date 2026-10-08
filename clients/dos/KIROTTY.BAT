@ECHO OFF
REM Dial Kiro TTY with MS-DOS Kermit 3.16 (see KIROTTY.INI).
REM Low memory? Replace MSK316 with KERLITE (no TCP/IP, smaller).
CD \KERMIT
MSK316 take kirotty.ini
