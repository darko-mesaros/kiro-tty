# Kiro TTY

It's Kiro for dumb terminals.

## How to run?

If you have `docker compose`, and `just`... Just run this:

```bash
git clone <repo> && cd kiro-tty
echo 'KIRO_API_KEY=ksk_...' > .env
just up
telnet localhost 2323     # kiro / kiro
```

