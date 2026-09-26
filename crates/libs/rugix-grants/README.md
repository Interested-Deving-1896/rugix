# Rugix Grants

A Rust library for signed, constrained grants authorizing typed device operations.
The library signs and verifies CMS envelopes using `rugix-pki`. Its public wire
contract is defined in [Sidex](schemas/grant.sidex).

The library provides:

- A versioned envelope binding an operation to a service, device or group, and
  validity window.
- Typed operations identified by a namespaced, versioned `Operation::TYPE`.
- Verification against a locally selected certificate authority, device identity,
  and explicit trusted time.
- Configurable envelope size and validity limits.
- Strict parsing that rejects unsupported fields, duplicate fields, and trailing data.

Use `prepare` for an external CMS signer, or `sign` with a `CmsSigner`. Call
`GrantVerifier::verify::<YourOperation>` with a `VerificationContext` containing
independently established identity and time. It returns an immutable
`VerifiedGrant<YourOperation>`.

Define operation payloads in Sidex and implement `Operation` for the generated
type. Use externally tagged variants to preserve unknown-field reporting through
nested payloads, and test rejection of unknown and duplicate constraints.
Change the type identifier when authorization semantics change.
The envelope schema is generic; operation payloads remain the executor's contract.
`rugix-bundle::grants::InstallOperation` is the installation implementation.

Verification authenticates the grant and checks common constraints. Before any
side effects, the executor must also:

1. Select an issuer authorized for that operation and resource.
2. Compare the entire authenticated operation with the requested action.
3. Enforce local policy and durable replay admission.
4. Define retries, activation, recovery, and any later validity checks.

The library deliberately owns no clock source, device identity source, persistent
state, network transport, or execution mechanism. A verified grant alone does not
prove that it is unused or locally authorized.

See [Detached Installation Grants](../../../docs/installation-grants.md) for the
Rugix workflow, trust assumptions, and wire format.
