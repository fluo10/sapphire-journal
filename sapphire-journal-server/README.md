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
unexpired token get `401 Unauthorized`. But a token by itself is not
enough — authentication is keyed off **devices**, not bare tokens. Every
key is minted for a device row, and any token whose device is missing from
`devices.toml`, retired, or expired is rejected exactly like a token that
was never issued. A key file carried over from before this device table
existed has none of its keys naming a live row, so every one of them gets
`401` (see [Migrating from `gen-key`](#migrating-from-gen-key) below).

Register a device and mint the key it authenticates with:

```sh
# optional: register who owns the device
sapphire-journal-server --journal-dir /path/to/your/journal user add --name fluo

# register the device and mint its token
sapphire-journal-server --journal-dir /path/to/your/journal device add --name laptop --user fluo
```

`user add` is optional — `device add` works fine without `--user` — but
naming an owner is useful once you have more than a couple of devices.
`--name` (both commands) is purely for your own bookkeeping; nothing in the
system reads it back to make an authorization decision. As with the old
`gen-key`, the command prints the token itself to stdout (so you can pipe or
copy just that line) and the device's id and creation time to stderr. Give
the printed token to the client as its `Authorization: Bearer <token>`
value. Add `--expires-in 90d` (or `12h`, `30m`) to `device add` if the key
should stop working on its own.

List devices, see which have a key on this host, and re-issue or stop one:

```sh
sapphire-journal-server --journal-dir ... device list
sapphire-journal-server --journal-dir ... device rotate laptop --expires-in 90d
sapphire-journal-server --journal-dir ... device retire laptop
```

`rotate` re-issues a device's token, keeping its id and its row. Its
`--expires-in` **replaces** the expiry rather than preserving it: omitting
the flag makes the new token non-expiring, it does not carry the old expiry
forward — re-issuing an already-expired key without a fresh `--expires-in`
does not restore its old deadline, it drops the deadline entirely. `retire`
revokes the device's key and marks its row retired rather than deleting it
outright, because device ids can end up written into content and a deleted
row would leave those references unresolvable (pass `--purge` if you want
the row gone anyway).

Both `rotate` and `retire` only change the files on disk: **a running
server keeps accepting the old token until it next reloads the device
table and key file** (in practice, until it restarts) — `ServerState` and
`DeviceAuth` hold a snapshot taken at start-up, with no path to reload it.
If you're rotating or retiring a device because its token leaked, restart
the server too, or the old token stays live.

### Where the files live

A key's token lives in a plain TOML file, one `[[key]]` table per key,
holding the token **in plaintext** — there is no hashing, because the
threat model is "this file lives on a private-network server," and hashing
would only cost the convenience of being able to read an existing key back
when setting up a new client. Treat this file the way you'd treat any other
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

The device and user tables (`devices.toml`, `users.toml`) live in the
opposite kind of place: `<journal-root>/.sapphire-journal/`, alongside the
journal's own config, where they **are** synced like any other journal
metadata. That's fine — a row only holds an id, a name, and an optional
description, never a secret. Splitting the two this way is what lets a
device's key live only on the machine that needs it while every synced
peer still sees the same device list.

## What "no usable key" means

A key file can exist and still leave the server with nothing usable in it
— empty, holding only keys that have expired, or (new since the device
table) holding keys whose device is missing, unregistered, or retired. In
any of these cases `serve` (the default, no-subcommand invocation) exits
immediately with an error before it opens a listening socket, rather than
starting up and quietly rejecting every request forever. Run `device add`
first if you see that error; it registers the device and mints it a key
against the same files the server just tried to read.

## Migrating from `gen-key`

If you set this server up before the device table existed, every token
`gen-key` issued now gets `401 Unauthorized` — none of them name a device,
and a keyless token authenticates to nothing. There's no migration path for
the tokens themselves; re-issue one per client with `device add` (as above)
and update each client's `Authorization` header to the new token. The old
entries in `keys.toml` are harmless but useless: the server logs a warning
at startup naming how many keys in the file authenticate to no device, and
you can delete those rows from `keys.toml` by hand once every client has
switched over.
