# Direct tree Web API rollout ledger

## Decision

Plant UI's model-tree structure is direct by default. It calls the model
service's `GET /api/v1/tree/{roots,children,ancestors}` endpoints; those
endpoints resolve `/ALL` from the SYS database and read the selected DESI files
through `e3d-io::ReadOnlyEngine`. No SurrealDB element/tree query participates
in these three operations.

`PLANT_TREE_DATA_MODE=db` is the explicit rollback switch. Geometry, properties,
search and other non-tree Plant UI features keep their existing stores; this
change only replaces the model-tree structure path.

## Dependency order and current ledger

1. **MDB scope** — resolve the configured project/MDB CURD from the SYS file.
2. **Direct affiliation** — walk each declared DESI live index with e3d-io and
   build `ref0 -> dbnum`; do not use the legacy whole-directory scanner.
3. **Tree contract** — SITE roots in CURD/member order, direct children in stored
   order, and self-first OWNER ancestors.
4. **Web contract** — every response carries `source: "direct"`; identity and
   malformed-refno requests fail closed.
5. **Plant UI** — direct Web API is the default; `db` is opt-in rollback.
6. **Acceptance** — compile both repositories, exercise all three production
   HTTP routes, traverse `/ALL`, and compare a complete DESI database to one
   E3D TTY session.

## Exit gates and evidence

- Server and Plant UI compile with zero errors.
- Production HTTP smoke returns `source=direct` for roots, children and ancestors.
- `/ALL` direct traversal completes without duplicate/cycle or OWNER mismatch.
- dbnum 8000 compares every displayed node's direct member list against E3D
  `Q MEMBERS`, preserving noun, identity/name and order; mismatch count is zero.
- Patch, verification manifest and rollback script are present and re-opened.
- Rollback script is executed against a staged copy and restores original hashes.

## Verification boundary

The E3D DESIGN TTY launcher updates the live database file's session metadata
even though the generated macro only selects elements and issues `Q MEMBERS`.
The db8000 before/after length stayed equal but SHA-256 and mtime changed. The
TTY result is therefore a live reference comparison, not an immutable-file
probe. The exact hashes and timestamps are retained in the evidence directory.

## Rollback

Run `.codex-artifacts/direct-tree-api/rollback.ps1`. It restores only the files
owned by this change from preserved originals and removes the two new direct-tree
files. For a behavior-only rollback without changing files, set
`PLANT_TREE_DATA_MODE=db` before starting Plant UI.
