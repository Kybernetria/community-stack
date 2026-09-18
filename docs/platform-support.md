# Native platform support

The native Community Stack core is currently **Linux-only**. This is an
intentional security boundary, enforced at compile time, rather than a claim
that every Unix-like platform has equivalent local-filesystem guarantees.
Linux CI is the supported build/test environment.

The native API uses Linux Unix-domain sockets, restrictive mode bits, local
filesystem locking, and private secret files. Open PR #1 (`Bound toolkit data
and secure local sockets`) further tightens socket binding with Linux
`openat2`/descriptor-relative operations. Its security properties must not be
replaced by a permissive pathname or loopback-TCP fallback.

Non-Linux Unix support would require a separate reviewed adapter providing
equivalent guarantees for socket creation/removal, peer isolation, atomic
private-file creation, permissions/ACLs, locking, and crash cleanup. It is not
claimed by this repository today. Android/mobile integrations should use a
platform IPC/FFI adapter as described in the architecture document, not enable
the Linux daemon or expose TCP.

This clarification avoids duplicating or weakening PR #1's Linux socket work.
A future portability change must add target-specific security tests and a CI
matrix before removing the compile-time guard.
