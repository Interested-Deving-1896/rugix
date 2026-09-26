# Detached Installation Grants

An installation grant authorizes a specific bundle for a device or provisioned group
during a limited time window. The grant is a separate CMS file. Issuing or renewing
it leaves the bundle unchanged, including its hash, streaming verification, and
delta delivery.

Rugix Ctrl verifies grants inside the installation executor. The CLI and privileged
daemon enforce the same grant policy. The signature covers the Rugix bundle hash,
device audience, validity window, authorization sequence, and installation options.

## Provision a Device

Configure `/etc/rugix/ctrl.toml`:

```toml
[grants]
roots = ["/etc/rugix/grant-root.pem"]
mode = { tag = "GrantOnly" }
namespace = "example-production"
device = "device-001"
groups = ["canary"]
trusted-system-clock = true
max-lifetime = 86400
```

The namespace, device ID, and groups are trusted provisioning data. Protect the
configuration and certificates from installation callers. Group names use exact
matching. Updating a group in an external inventory does not update this local
membership automatically.

Set `trusted-system-clock = true` only when the platform establishes trustworthy
current time across power cycles, for example through a protected clock or an
authenticated time service. The same time is used for grant and certificate
validity. Grant timestamps and CMS signing-time are not time sources. A stored
timestamp alone cannot account for time spent powered off. With this option false,
Rugix refuses granted installations.

Grant replay state uses `/run/rugix/mounts/data/.rugix/grants` when Rugix state
management is active, detected by the presence of `/run/rugix/state`. This location
survives a state-profile reset. Systems without state management use
`/var/lib/rugix/grants`. An optional `state-directory` setting overrides the path.

Initialize state after the device's storage and state management are set up. Keep
the selected directory on persistent, protected storage outside the A/B system
slots and resettable profiles. When changing the storage layout, migrate the
existing replay state. Rugix fails closed if the selected state file is missing,
invalid, or belongs to another provisioned identity; it does not search other
locations for a usable state file. Do not automatically initialize missing state
at boot. A full data-partition wipe removes grant history and requires explicit
reprovisioning.

Initialize state once during provisioning, as root:

```sh
rugix-ctrl initialize-grant-state
```

Initialization refuses to overwrite existing state. Restoring an old state backup
can restore old permissions; deployments defending against storage rollback need
hardware-backed protection for their security state and verifier.

With a `[grants]` section present, every system and app installation requires a
grant. Caller-supplied bundle hashes, root certificates, compatibility overrides,
and insecure verification options cannot bypass this requirement. The daemon's
`dangerously-insecure` switch does not override grant policy.

Grant roots authorize both system and app installation on the configured device.
A grant issuer can authorize any bundle under `GrantOnly`. Use independent publisher
verification when deployment authorities should only select publisher-approved
software:

```toml
[signatures]
roots = ["/etc/rugix/publisher-root.pem"]

[grants]
roots = ["/etc/rugix/grant-root.pem"]
mode = { tag = "EmbeddedAndGrant" }
namespace = "example-production"
device = "device-001"
trusted-system-clock = true
```

Both signatures are then mandatory. Adding several roots allows certificate
rotation within one authority; any accepted grant root may issue a grant. Restart
the daemon after changing its configuration.

## Issue and Install a Grant

Issue a grant on a trusted signing machine. Validity timestamps are Unix seconds,
with an inclusive start and exclusive end.

```sh
now=$(date +%s)
rugix-bundler grants sign \
  --bundle update.rugixb \
  --id rollout-42-device-001 \
  --namespace example-production \
  --device device-001 \
  --not-before "$now" \
  --expires-at "$((now + 3600))" \
  --sequence 42 \
  --target system \
  --reboot set \
  --cert grant-signer.pem \
  --key grant-signer.key \
  update.cms
```

Install with the same authorized options:

```sh
rugix-ctrl update install --grant update.cms --reboot set update.rugixb
```

Use `--group canary` instead of `--device device-001` for a provisioned group.
Use `--target apps` when signing for `rugix-ctrl apps install --grant app.cms app.rugixb`.

System grants bind `--boot-group`, `--keep-overlay`, and `--reboot` exactly.
Omitting a boot group authorizes local selection of an inactive group. Omitting
reboot behavior authorizes the bundle's default. System options cannot be used
with an app grant. The bundle hash binds its payload destinations, including app
names.

The default maximum grant size is 1 MiB, including certificates. The default
maximum validity window is one day; `max-lifetime` configures the device's limit.
The verifier also applies normal bundle integrity, destination, and compatibility
checks.

Inspect authenticated grant content and compare it with a trusted bundle:

```sh
rugix-bundler grants verify update.cms \
  --root-cert grant-root.pem \
  --namespace example-production \
  --device device-001 \
  --bundle update.rugixb
```

For group grants, also supply the independently established membership with
`--group`. This command uses the local clock and the library's default limits.
It does not inspect a device's replay state or authorize an installation.

## Use an External Signer

Prepare the exact bytes that must be signed, using the same grant options as above:

```sh
rugix-bundler grants prepare \
  --bundle update.rugixb --id rollout-42-device-001 \
  --namespace example-production --device device-001 \
  --not-before "$now" --expires-at "$((now + 3600))" \
  --sequence 42 --target system --reboot set grant.raw

openssl cms -sign -binary -nodetach \
  -in grant.raw -signer grant-signer.pem -inkey grant-signer.key \
  -outform DER -out update.cms
```

The CMS envelope must include the signed content. Its signing certificate and
intermediate chain must validate against a configured grant root. The existing
Rugix PKI certificate rules apply, including digital signature key usage and
code-signing extended key usage when present. The signing command also supports
repeated `--intermediate-cert` arguments.

## Replay, Expiry, and Recovery

Rugix maintains separate authorization sequences for system installations and app
installations. All apps share one sequence stream. An issuer must coordinate
increasing sequences across its keys and device or group grants within each stream.
Sequence numbers describe authorizations, so an intentional downgrade uses a newer
sequence for an older bundle.

After preflight, Rugix durably reserves the grant before installation side effects.
An interrupted transfer can retry the exact same grant while it remains valid.
Changing its ID or content at the same sequence is rejected. A newer reserved
authorization supersedes older ones.

Before activating apps, selecting or deferring a system boot, or finalizing a staged
system update, Rugix rechecks grant and certificate validity and durably consumes
the grant. Once consumed, another installation requires a higher sequence, including
when activation fails or power is lost between consumption and activation.
This permits retries of incomplete transfers and prevents repeated activation
admission. It does not promise exactly-once execution of arbitrary payload handlers.

An update that expires while streaming cannot proceed to activation. Its inactive
data may remain and can be replaced by an installation with a new grant. Manual
app activation, manual app rollback, and `system reboot --spare` are disabled under
grant policy. To select stored software again, install its bundle with a new grant.
A staged system installation with `--reboot no` follows the same rule.

Once activation is durably authorized, boot retries, commit, and automatic recovery
may finish after expiry. Existing software continues running. Deferred reboot
authorization may execute on a later boot. The validity window limits authorization
of the operation, not the time at which software must stop running.

Short validity windows limit how long an offline grant remains usable. Immediate
revocation requires fresh information on the device. Protect the privileged
verifier, local policy, clock, and replay state as part of the device security boundary.

## Library and Wire Format

`crates/libs/rugix-grants` provides the reusable envelope, CMS signing, and
verification for typed operations. It has no installer, filesystem, transport, or
daemon dependency. `rugix-bundle` defines the `rugix.install.v1` operation and its
`rugix-ctrl` verifier audience. The executor owns resource policy, replay state, and
operation recovery.

The signed CMS content is the byte prefix `rugix.operation-grant.v1\0`, followed by
UTF-8 JSON. The signature covers the original bytes. Verification does not
reserialize JSON. Unsupported versions, operation types, unknown fields, duplicate
fields, and trailing content are rejected. A grant cannot be substituted for
ordinary embedded bundle metadata.

The Sidex source contracts are
[`grant.sidex`](../crates/libs/rugix-grants/schemas/grant.sidex) and
[`grants.sidex`](../crates/libs/rugix-bundle/schemas/grants.sidex).
The unsigned 64-bit fields `notBefore`, `expiresAt`, and `sequence` accept JSON
integers or decimal strings. Sidex emits decimal strings for values above
JavaScript's maximum safe integer, 9007199254740991.

## Verify Changes

Run the Rust checks and the CLI/daemon test:

```sh
mise run check
mise run test:grants
```

The end-to-end test needs Linux user and mount namespaces, Python, OpenSSL, and
`mount`. It replaces device paths in private namespaces and does not require host
root access. It exercises real bundle creation, CMS issuance, app activation,
system installation to file slots, boot selection through a test controller,
streaming expiry, interrupted transfer recovery, replay protection, daemon policy,
independent signing authorities, and external OpenSSL signing.
