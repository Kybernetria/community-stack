# Community Planning interoperability profile v1

- Profile ID: `community.planning`
- Version: `1`
- Data namespace: `community.planning`
- Canonical manifest: [`community-planning-v1.json`](community-planning-v1.json)
- BLAKE3 manifest digest: `771d32c6a851318c84139c924c573eac69d9ab8c7edca4f179afe344c48879ee`

The JSON file is the byte-canonical profile identity. Whitespace or semantic changes require a new version and digest. Tests pin the exact bytes. This profile standardizes signed record meaning, not application database layouts or user interfaces.

## Authority and access

Profile records use `community.planning` as the signed operation `app_id`/data namespace, independently of the calling APP principal. The encrypted signed Loro record contains:

```json
{
  "profile_id":"community.planning",
  "profile_version":1,
  "profile_digest":"771d32...879ee",
  "submitted_by_app_id":"org.example.calendar",
  "record":{}
}
```

A local ADMIN must grant each APP read or read/write access for each specific community with `profile.grant`. Write implies read. These grants enforce local community isolation, but are not replicated membership or p2panda-auth authorization. APP tokens remain distinct, idempotency keys remain scoped to the caller, and the data namespace is reserved from principal registration, and generic document methods cannot enter it. Accepted signed operations are authoritative; planning SQL tables are rebuildable views.

## Closed record types

Version 1 has four record types and rejects unknown request fields:

- **Project:** ID, title, optional description, and status.
- **Task:** ID, optional project, title/description, status, optional start/due UTC epoch milliseconds, and progress percentage.
- **Event:** ID, optional project, title/description, status, timing, bounded recurrence, and optional location.
- **Dependency:** ID, predecessor/successor tasks, dependency kind, and bounded lag.

Statuses are `planned`, `active`, `completed`, `cancelled`, and `archived`. Dependency kinds are `finish_to_start`, `start_to_start`, `finish_to_finish`, and `start_to_finish`. Self-dependencies, missing task references, and cycles fail closed.

The v1 extension policy is closed. Namespaced extensions require a later profile version; applications must not reinterpret standard fields. This deliberately prioritizes interoperable meaning over arbitrary per-record schema freedom.

## Temporal semantics

All-day events use calendar dates and an exclusive end:

```json
{"kind":"all_day","start_date":"2026-08-10","end_date_exclusive":"2026-08-11"}
```

Timed events use canonical UTC Unix epoch milliseconds plus an IANA-style zone identifier used only for presentation:

```json
{"kind":"utc","start_at_ms":1786352400000,"end_at_ms":1786356000000,"time_zone":"Europe/Berlin"}
```

The core validates identifier syntax but does not embed or validate against a TZDB release in v1. Timed recurrence uses UTC arithmetic, not zone-local wall-clock arithmetic; this is deterministic across TZDB releases but means a recurring local display time can shift at daylight-saving transitions. A production iCalendar adapter must expose that limitation, pin its TZDB policy, and preserve original `VTIMEZONE` information where necessary.

Recurrence supports bounded `daily`, `weekly`, `monthly`, and `yearly` rules with interval, exactly one of count or UTC `until_ms`, and at most a ten-year timed horizon. `by_weekday` is valid only for weekly rules; an empty list means the start weekday. All-day recurrence uses count in v1. Daily rules advance by UTC days or calendar dates, weekly rules use weekdays, monthly rules retain the start day and skip months without it, and yearly rules retain month/day and skip invalid dates. Recurrence instances are derived views, never authoritative rows. Full RFC 5545 RRULE parity, exceptions, detached instances, attendees, alarms, and CalDAV synchronization are deferred.

## Views

`planning.calendar.list` and `planning.gantt.get` are independent bounded views over the same signed records. They return `resolved_truth:false`: planning records are shared assertions/state, and future group policy or conflict UX may be needed for concurrent edits.

## Conformance requirements

An implementation claiming this profile must:

1. match the canonical manifest digest;
2. reject malformed temporal values, unknown request fields, missing references, and dependency cycles before durable writes;
3. transactionally compare the observed community projection generation so independent local cores cannot violate cross-record invariants;
4. bind profile ID/version/digest and submitting APP into each signed record;
5. use the shared profile namespace rather than caller-private storage;
6. preserve exact idempotency and atomic operation/update/projection/outbox commits;
7. expose the same records to independently granted calendar and Gantt clients;
8. never treat SQL projection rows as replicated authority.
