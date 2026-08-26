# sapphire-journal-server

`sapphire-journal-server` is a self-hosted server that exposes one
`sapphire-journal` journal to other machines: a sync API for clients that
want to pull and push changes (`/rpc`), and an MCP endpoint for AI agents
that want to read and write entries directly (`/mcp`). Both routes run from
a single process over a single journal, so a write made through either one
shows up to the other — a `/rpc` client's pull will see an entry an agent
just created over `/mcp`, and vice versa.

## Before you run this: it is private-network only

This server has **no TLS and no OAuth**. Every request goes over plain
HTTP, and the only thing standing between an entry and anyone who can reach
the port is a bearer token compared in memory. That is an intentional
trade-off for something meant to run on a home server, a NAS, or a small
box reachable only over your own VPN or [Tailscale](https://tailscale.com/)
— not something to expose on the open internet. Run it behind a private
network, and let that network do the job TLS and OAuth would otherwise do.
The server's own startup log repeats this assumption every time it starts,
specifically so an operator watching the log sees it, not just this file.

## Running it

```sh
sapphire-journal-server --journal-dir /path/to/your/journal
```

By default it binds `127.0.0.1:8080`; pass `--addr 0.0.0.0:8080` (or
whatever your private network needs) to listen more broadly. `--journal-dir`
can also come from the `SAPPHIRE_JOURNAL_SERVER_DIR` environment variable,
and `--addr` from `SAPPHIRE_JOURNAL_SERVER_ADDR`, which is usually more
convenient for a service unit or a container than repeating flags.

### If you widen `--addr`, name the hostnames too

Binding beyond loopback is only half the job. `/mcp` sits behind rmcp's
DNS-rebinding guard, which checks the `Host` header of every request against
an allowlist that starts out holding loopback and the bind address only. A
client that reaches the server as `http://box.tailnet.ts.net:8080/mcp` sends
`Host: box.tailnet.ts.net:8080`, which is on no list, and gets `403
Forbidden` — no matter how valid its token is.

So name each hostname your clients actually type:

```sh
sapphire-journal-server --journal-dir /path/to/your/journal \
  --addr 0.0.0.0:8080 \
  --allowed-host box.tailnet.ts.net --allowed-host nas.local
```

`--allowed-host` is repeatable, accepts either `host` or `host:port` (the
bare form matches any port), and can also come from
`SAPPHIRE_JOURNAL_SERVER_ALLOWED_HOSTS` as a comma-separated list. Loopback
stays allowed whatever you pass, so adding names never breaks a local
client.

**What it looks like when you forget:** sync keeps working and only the AI
side breaks. `/rpc` has no `Host` check, so a synced client pulls and pushes
happily while every MCP client gets `403 Forbidden: Host header is not
allowed` — often reported as "the agent can't see my journal" rather than as
a server problem. The server logs a warning at startup when it binds beyond
loopback with no `--allowed-host`, and rmcp logs `rejected request with
disallowed Host header` for each such request; the startup line also prints
the full allowlist it ended up with.

The server refuses to bind at all if it has no usable API key configured —
see the next section. There is no "start it open and lock it down later";
an unauthenticated listener is never a state this process will sit in.

## Keys

Clients authenticate with a bearer token: every request to `/rpc` or `/mcp`
must carry `Authorization: Bearer <token>`. Requests without a valid,
unexpired token get `401 Unauthorized`.

Issue a key with the `gen-key` subcommand, run against the same journal:

```sh
sapphire-journal-server --journal-dir /path/to/your/journal gen-key laptop
```

The label (`laptop` here) is optional and purely for your own bookkeeping —
nothing in the system reads it back. The command prints the token itself to
stdout (so you can pipe or copy just that line) and the key's id and
creation time to stderr. Give the printed token to the client as its
`Authorization: Bearer <token>` value. If you lose it, you don't have to
issue a new one — it's sitting in plaintext in the key file (see below), so
you can read it back out and set up another client with the same token. Add
`--expires-in 90d` (or `12h`, `30m`) if the key should stop working on its
own.

`sapphire-journal-server --journal-dir ... list-keys` lists the keys that
exist, with tokens masked, so you can see what's issued without printing
live secrets again. `sapphire-journal-server --journal-dir ... revoke-key
<id-or-label>` removes one.

### The key file

Keys live in a plain TOML file, one `[[key]]` table per key, each holding
its token **in plaintext** — there is no hashing, because the threat model
is "this file lives on a private-network server," and hashing would only
cost the convenience of being able to read an existing key back when
setting up a new client. Treat this file the way you'd treat any other
plaintext secret: readable only by whoever runs the server.

By default the key file lives in your OS cache directory, under
`sapphire-journal/<id>/keys.toml`, where `<id>` is a UUID derived from the
journal's own filesystem path (e.g. `~/.cache/sapphire-journal/<id>/` on
Linux, `%LOCALAPPDATA%\sapphire-journal\<id>\` on Windows) — **entirely
outside the journal root**, so a key can never end up inside the synced
content and get shipped to a client the way an entry would. That `<id>` is
purely a cache-directory name; it is not the same id clients use to name
this workspace when they sync. Pass `--keys /some/other/path` (or
`SAPPHIRE_JOURNAL_SERVER_KEYS`) to put the key file somewhere else
entirely, such as a location your backup policy excludes on purpose.

## What "no usable key" means

A key file can exist and still leave the server with nothing usable in it
— empty, or holding only keys that have expired. In either case `serve`
(the default, no-subcommand invocation) exits immediately with an error
before it opens a listening socket, rather than starting up and quietly
rejecting every request forever. Run `gen-key` first if you see that error;
it's the same file the server just tried to read.
