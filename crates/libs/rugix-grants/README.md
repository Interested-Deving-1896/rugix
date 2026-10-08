# Rugix Grants

A Rust library for signed, constrained grants authorizing typed operations. It signs
and verifies CMS envelopes using `rugix-pki`. The public wire contract is defined in
Sidex: [grants](schemas/grant.sidex).

The library provides:

- A versioned envelope binding an operation to a service, a recipient or group, and
  a validity window.
- Typed operations identified by a namespaced, versioned `Operation::TYPE` and the
  key purpose that authorizes them.
- Verification against one locally authorized certificate authority, the permissions
  local policy delegates to it, an independently established recipient identity, and
  explicit trusted time.
- Certificate-bound scope limiting which namespace and audiences a signing key may
  address, enforced across delegation.
- Strict parsing that rejects unsupported fields, duplicate fields, and trailing data.

## Usage

Define the operation payload in Sidex and implement `Operation` for the generated
type, returning the object identifier that authorizes it. Operations whose arguments
need separate authority return separate identifiers, so an authority can be limited
to one of them. Use externally tagged variants to preserve unknown-field reporting
through nested payloads, and test rejection of unknown and duplicate constraints.
Change the type identifier when authorization semantics change.

Use `prepare` for an external CMS signer, or `sign` with a `CmsSigner`. Construct a
`GrantVerifier` for one trust root and the permissions local policy allows it, then
call `GrantVerifier::verify::<YourOperation>` with a `VerificationContext` holding the
independently established identity and time. It returns an immutable
`VerifiedGrant<YourOperation>`.

`rugix-install-grants` is the reference operation contract.

## Certificates

Only certificates prepared as grant authorities can sign. Every certificate below the
trust anchor carries a critical extended key usage with the grant authority purpose
plus one purpose per operation it may authorize, and a non-critical extension holding
a DER `authority::AuthorityScope`. Use `authority::purposes_extension_der` and
`AuthorityScope::to_extension_der` to build them, or generate OpenSSL extensions with
`rugix-bundler grants authority-extensions`.

Delegation depth and certificate validity use standard X.509 basic constraints and
validity periods, which the path verifier enforces for every certificate in the
chain. Only the namespace and audience selectors are checked by this library.

## Responsibilities

Verification authenticates a grant and checks its certificate-bound scope. Before any
side effects, the executor must also:

1. Compare the entire authenticated operation with the requested action.
2. Enforce its own resource policy.
3. Durably enforce its replay policy.
4. Define retries, activation, recovery, and any later validity checks.

The library deliberately owns no clock source, recipient identity source, persistent
state, network transport, or execution mechanism. A verified grant alone does not
prove that it is unused or locally authorized.

See [Detached Installation Grants](../../../docs/installation-grants.md) for the wire
format, the certificate profile, and the assigned identifiers.
