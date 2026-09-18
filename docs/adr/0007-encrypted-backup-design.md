# ADR 0007: Versioned encrypted device backups (design proposal)

- Status: proposed
- Date: 2026-09-18

## Decision status

Do not implement the encrypted format in the current change set. The existing
v1 backup format and restore path remain compatible. This proposal freezes the
security and recovery decisions that must be reviewed before adding a v2
writer, CLI key handling, and migration tests.

## Threat model

A backup may be copied from removable media, a cloud-sync directory, or a lost
device. An attacker with the backup must not obtain the device signing
identity, content master key, database contents, or plaintext operation history
without the backup secret. The attacker may truncate, reorder, replace, or
modify bytes and metadata. Availability is not guaranteed: deletion and
resource exhaustion remain outside the confidentiality goal.

The backup secret is not a device identity and is not recoverable from the
backup. Losing it makes the encrypted backup unrecoverable. A passphrase is
also not a group-membership or replication authorization mechanism.

## Proposed v2 format

v2 is a versioned encrypted bundle directory, distinct from the existing
`community-stack-device-backup` v1 directory. It contains a small cleartext
format header and one encrypted streaming payload. The header contains only:

- magic and format version (`community-stack-device-backup`, version `2`);
- cipher/KDF identifiers and bounded parameters;
- a random 16-byte KDF salt;
- a random 16-byte stream nonce prefix;
- the fixed chunk size and format flags.

It does not contain keys, plaintext records, or a plaintext file manifest. The
payload is a sequence of length-delimited encrypted chunks followed by a
required encrypted manifest footer. Each chunk records the logical file name,
file offset/chunk number, plaintext length, and ciphertext. Its XChaCha20-
Poly1305 associated data is the canonical header plus those record fields.
The final footer contains the ordered file names, byte sizes, and BLAKE3
content digests, and is authenticated with a separate nonce and associated
data `canonical_header || "manifest"`. A restore rejects missing footers,
unknown records, duplicate/out-of-order chunks, size-limit violations, and
manifest/hash mismatches.

The authenticated footer prevents a checksum-only replacement of the manifest.
Per-chunk AEAD detects modification before large plaintext is accepted. BLAKE3
is retained as a deterministic content check and for the existing canonical
history validation; it is not used as an authentication substitute.

## KDF, cipher, salt, and nonce policy

- KDF: Argon2id, versioned by the format, with 32-byte output, memory `64 MiB`,
  three iterations, and one lane in the initial profile. Parameters are stored
  in the clear header and authenticated by every record; restore accepts only
  reviewed bounded ranges (no attacker-selected unbounded allocation).
- Encryption: XChaCha20-Poly1305, already used by the object-encryption adapter
  and pinned in `Cargo.toml`.
- The salt is generated from the OS CSPRNG for every backup and is never reused
  as a key or nonce. The 16-byte stream prefix plus a checked 64-bit chunk
  counter yields a unique 24-byte chunk nonce for one derived key. The manifest
  footer uses an independent random 24-byte nonce and domain-separated
  associated data. Counter overflow, duplicate counters, and nonce generation
  failures fail closed.
- A future implementation must add and pin an audited Argon2 dependency rather
  than silently substituting a fast hash or changing the existing object
  encryption parameters.

## Manifest authentication and streaming

The writer first obtains the SQLite image through the existing SQLite online
backup API. It then streams the bounded key files and database into encrypted
chunks while calculating the footer digests. No plaintext copy is written to
the user-visible backup destination. SQLite may require a private staging file
for its online-backup API; if so it is created mode `0600` in a mode `0700`
private staging directory, fsynced, bounded, and removed on success or failure.
It is never named as a finished backup entry.

Chunk sizes are fixed and bounded (the initial proposal is 1 MiB); the total
backup size and each logical file have explicit limits. Restore decrypts one
chunk at a time into private staging files, verifies the authenticated footer
and all digests, then runs the existing SQLite integrity, foreign-key, schema,
canonical-operation, identity, and Loro-history checks. It never loads the
whole database or encrypted payload into memory.

## Atomic creation and failure behavior

Creation uses the existing incomplete-marker lifecycle: make an exclusive
mode `0700` destination directory, create and fsync an incomplete marker, write
only private mode `0600` payload/staging files, fsync the payload and directory,
then remove the marker and fsync the directory again. Existing destinations,
symlink entries, and unsafe parents are rejected; no overwrite mode is
provided. A destination with an incomplete marker is never accepted by verify
or restore.

Restore requires an absent destination, creates it with the incomplete marker,
and leaves that marker on any authentication, wrong-key, integrity, schema, or
history failure. It never replaces an existing directory and never promotes
partially verified files. Cleanup of a failed incomplete destination is an
explicit local filesystem operation, not an implicit destructive recovery.

## Passphrase and key handling

The CLI must not accept passphrases in argv, logs, API errors, or ordinary
environment variables. The eventual interface should support an interactive
no-echo prompt and an explicitly selected file descriptor/OS-keychain source;
any passphrase file must already be a regular mode `0600` file and must not be
a symlink. Input is bounded, compared without logging, and cleared from memory
as far as the selected dependency permits. A hardware/organization key
wrapper can be added later as a separate versioned key source; it must not
change the backup payload format.

## Backward compatibility

Existing v1 backups remain readable by `verify` and `restore` so current
recovery procedures do not break. v1 remains explicitly documented as
plaintext-at-rest: checksums detect accidental/correlated modification but do
not provide confidentiality against possession of the directory. New backup
creation defaults to v2 once implemented; v1 creation is not silently
reintroduced. A future CLI should offer an explicit v1-to-v2 migration that
reads and validates v1, writes a new v2 destination, and leaves the source
untouched.

## Required implementation gates

Before implementation, add fixed vectors for header canonicalization, Argon2id
parameters, chunk boundaries, nonce counters, wrong-password behavior, footer
truncation, tampering, reordering, oversized lengths, interrupted creation,
and v1 restore compatibility. Add crash/interruption tests at each fsync and
marker transition. The implementation must preserve the single SQLite
authority, online-backup semantics, private-key/content-key separation, and
redacted logging guarantees. A cryptographic review is required before v2
becomes the default backup format.
