# Midenup

`midenup` manages toolchains: installing, updating, uninstalling, and deciding which one is active.

## The midenup directory

Everything lives under one directory, `$XDG_DATA_HOME/midenup` by default (see the [XDG base directory spec](https://specifications.freedesktop.org/basedir-spec/latest/#variables)), overridable with `MIDENUP_HOME`.

```text
$MIDENUP_HOME/
├── state.json                          what is installed; the sole logical authority
├── .lock                               advisory lock; guards mutating operations only
├── channel-manifest.json               the last upstream manifest fetched, cached verbatim
├── journal/
│   └── <operation-id>.json             present only while an operation is in flight
├── publications/
│   └── <channel>-<publication-id>/     immutable
│       ├── receipt.json                what this publication owns
│       ├── bin/  lib/  etc/  opt/
├── var/
│   └── <channel>/                      mutable user data; never touched by install or update
├── toolchains/
│   ├── <channel>  -> ../publications/<channel>-<publication-id>
│   ├── <network>  -> <channel>            one link per network naming an installed channel
│   └── default    -> <channel> | <network>
└── opt -> toolchains/<active-channel>/opt
```

There is one `<network>` link per network — `mainnet`, `testnet`, `devnet` — and it exists only once
that network's channel is installed here, so it can never dangle. Several of them may name the same
channel. The link records the last answer upstream gave that this machine acted on, which is what
lets `miden` name the active channel offline.

Two things worth noting:

**A publication is immutable.** It is written once, verified, published, and never modified. Changing what is installed produces a *new* publication and repoints one symlink. Nothing infers anything from a publication's name: the id is opaque and randomly generated, precisely so that no code can be tempted to treat two equal names as two equal directories.

**`var/` is outside the publication.** It holds mutable component-owned state — most importantly the Miden client's database, referenced from the manifest as `%var(data)`. Install, update and republication never read, write, move or delete it. The one exception is channel migration, which renames* `var/<old>` to `var/<new>` so client data follows the toolchain it belongs to.

## Installation

Installation is not atomic — it touches a staged tree, a symlink, `state.json`, derived symlinks and the previous publication — so it is made **recoverable** instead, with a single decision point that decides whether it happened:

```text
1. PREPARE   write journal/<op-id>.json: what is about to happen
2. STAGE     build publications/<channel>-<new-id>/, seeding from the old receipt
3. VERIFY    structural check; write receipt.json
4. COMMIT    repoint toolchains/<channel>            <- the commit point
5. RECORD    commit state.json
6. DERIVE    repoint every toolchains/<network> naming this channel, and opt
7. CLEAN     release the old publication; delete the journal
```

Recovery runs at startup. Before step 4 the operation never happened: the staged publication is discarded and prior state stands. After step 4 it did happen: steps 5–7 are completed from the journal. Which side of the commit point an operation is on is read from the symlink and nothing else. Uninstall replaces step 4 with an atomic replacement of the symlink by a tombstone, so that a committed removal is distinguishable from a toolchain someone deleted by hand.

Seeding is scoped by the previous publication's **receipt**: only files it owns, only those the new plan still wants, and never those of a component known to have changed. That is what makes an update cheap without letting it carry stale content forward.

Verification before publication is **structural**: every planned file exists, is a regular file, and carries the planned mode. Contents are not checked. A recorded digest is not evidence of anything (see [Manifest](./manifest.md)), so claiming otherwise here would be dishonest.

### Concurrency

Mutating operations take an exclusive `flock` on `$MIDENUP_HOME/.lock`; read-only ones take nothing. This is not optional: `miden <cmd>` installs the current toolchain if it is missing, so two shells in two project directories are two concurrent writers with no user error involved. A blocked writer says so after a second and waits up to ten minutes. On acquiring the lock it re-reads `state.json`, because whoever held it may have changed what is installed.

### Reclamation

A publication that has been *replaced* is left on disk. Another process may still be executing a component out of it, and pulling the directory out from under a running program is fatal. Since the toolchain symlink now points elsewhere, nothing can start using it — it is simply unreferenced, and `midenup gc` reclaims those.

## Update

Update decides two things and delegates the rest:

1. **Which components must be re-acquired.** A component needs reinstalling when its contribution to the installation's plan key changed — its authority, kind, installation method, artifacts, destinations, modes, Cargo features, rustup channel or symlink layout. Changes to `requires` or `profiles` change what is *selected*, not what an unchanged component looks like; changes to aliases, call formats or `initialization` change neither.
2. **Which selection the result is recorded under.** An update re-resolves the *persisted* intent against the new upstream channel, so a `minimal` installation gains components newly tagged `minimal`, and a roots-only installation gains new dependencies of its roots but not unrelated new profile members.

Everything else — resolving, planning, staging, publishing — is the same code path as a fresh install. A change that touches only selection or runtime metadata skips it entirely and commits a single `state.json` write: republishing an identical tree to record an alias would be pure cost.

**Updating a network reconciles the pointer, not the channel.** `midenup update mainnet` looks at what the manifest now has `mainnet` naming. If that has moved, the installation is carried there: the recorded selection transfers verbatim and is re-resolved against the channel now being tracked, `var/` is renamed so client data follows the network rather than being stranded under a version nobody is tracking any more, and the `toolchains/mainnet` link is repointed last. The comparison is inequality rather than "is newer" — the pointer is authoritative in both directions — so a promotion that moves a network *back* is followed too, with a warning that data written by a newer toolchain is being carried across as-is.

An explicit root that no longer exists upstream **blocks** the update and preserves the installation. The schema has no rename declaration, so guessing is not an option.

Components installed from a local path are held back by default, since rebuilding them is potentially destructive; `--path-update=all` or `--path-update=interactive` changes that. A held-back component keeps the definition it was installed with, so the next update still offers.

## Activation and ownership

A channel has **one** installed publication holding the union of everything asked for. A project's `miden-toolchain.toml` adds its requirements to that union; it never removes what another project asked for. An explicit `midenup install <channel> --profile <p>` replaces the selection outright and is therefore the documented way to shrink one back to a known set.

What a project *sees* is narrower than what is installed: the active view is that project's request resolved against this machine's installation. Command discovery and alias resolution use it. A component that is installed but outside the view still runs when named explicitly, with a warning — and an alias that is ambiguous only across the whole installed superset is a warning too, rather than something that breaks every command.
