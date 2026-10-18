# Development History

[Project overview](../../README.md) · [简体中文](../README_ZH.md) · [Changelog](../CHANGELOG.md)

## What Changed

On 2026-10-18, the owner approved consolidating `nightly` into version and
development-phase snapshots. Its 2,252 original commits remain available in the
[complete original nightly archive](https://github.com/nowaytouse/modern-format-boost/commits/archive/nightly-before-phase-squash-20261018-589b5921).

The consolidated line retains the original root, 13 snapshots and the subsequent
documentation cleanup commit: 15 commits at this boundary. This is a one-time
history change, not a release, algorithm rollback or deletion of development work.
Later work adds normal commits; the number is not a permanent limit.

The archive ends at `589b5921e343a55c26c9edab28d8e3b9cb40939b`.
Its branch is `archive/nightly-before-phase-squash-20261018-589b5921`.
Existing version tags and `main` were not rewritten. The historical
`v6.4.6-pre-refactor` label is retained as a historical phase name, not reinterpreted
as part of the current version sequence.

## Snapshot Map

Each consolidated snapshot has exactly the same tracked file tree as its original
source commit. Commit messages carry the full `Original-Snapshot`,
`Original-Range` and `Original-History` references. Original authorship, individual
changes, signatures and prior CI records belong to the archived commits; the new
phase commits do not reproduce those signatures or claim new test results.

| Version or phase endpoint | Original snapshot | Consolidated snapshot |
| --- | --- | --- |
| Original project root, unchanged | `ba77a9d1` | `ba77a9d1` |
| v0.2.0 media pipeline baseline | `40e9b0af` | `a4824e03` |
| Historical pre-refactor phase | `40eaeb61` | `2cfc8f1f` |
| v0.10.66 media compatibility | `2a2a99c8` | `1589c35b` |
| v0.11.0 verification and interface | `e23614a7` | `0c692fe5` |
| v0.11.2 processing baseline | `ae4c99d9` | `79592217` |
| v0.12.0 native history baseline | `faffaa12` | `baf8f7e1` |
| Configuration, progress and cache accounting | `c12ebf9b` | `c312befd` |
| Scan evidence, ownership and failure policy | `27f597cc` | `852f56b2` |
| Verification evidence, backups and shared workers | `62830fdf` | `e3c97f17` |
| Preflight and IMG algorithm reference | `a8df1539` | `35351902` |
| IMG policy, search settings and safe recovery | `e59caf76` | `24308a12` |
| Native guide, diagnostics and cleanup receipts | `cf848853` | `85324cfc` |
| Gate isolation, exhaustive JPEG search and result reporting | `589b5921` | `14e84b43` |

The final documentation cleanup follows these snapshots as an ordinary commit.
Omitted patch tags remain available at their original targets; selecting phase
endpoints does not move tags or erase the intervening archived commits.

## Algorithm References Still Work

The established IMG algorithm reference remains the original archived commit:

```text
a8df153932f34869a7871108b30c759bca1d4605
```

Its equivalent consolidated file tree is `35351902`. Existing references to
`dea69c6b`, `62830fdf`, `fd2bb2de` and `1041fe54` still resolve in the archived
history. Do not replace historical comparisons with a nearby phase merely because
it has a similar name: only mapped snapshots have an identical-tree guarantee.

With the archive fetched, these are read-only ways to inspect the reference:

```bash
git show a8df153932f34869a7871108b30c759bca1d4605:crates/foundation/src/image/jxl_explorer.rs
git diff a8df153932f34869a7871108b30c759bca1d4605 35351902 --
```

The second command must produce no differences. The last pre-cleanup snapshot also
retains tree `d8eb67b8c3a8e52e6a6036523c0b17e049ab5d58`; the subsequent README,
changelog and history-guide edits are intentional documentation changes.

## Existing Clones And Future Work

- Fetch the updated remote before continuing work. An old local `nightly` and the
  consolidated remote have different histories; an ordinary pull may propose an
  inappropriate merge. Preserve local work on a separate branch before realigning.
  A fresh clone is the simplest option when no local work needs to be carried over.
- Do not merge the archive or old `main` back into consolidated `nightly`; that
  would reintroduce the long history. Port a needed change deliberately instead.
- Do not delete the archive as routine cleanup. It preserves original attribution,
  algorithm references and the detailed evidence needed for older investigations.
- Keep README focused on onboarding, stable behavior in the relevant guides, and
  version changes in [CHANGELOG](../CHANGELOG.md). Group future commits around
  coherent, validated changes rather than individual edit or test attempts.
- Prefer consolidating development-branch work before integration. Rewriting an
  already published branch again requires explicit approval, an archive and the
  same snapshot checks; it is not an automatic maintenance operation.

The default branch's commit list is shorter, but the archive intentionally keeps
the original Git objects. This is a readability improvement, not a disk-space or
repository-size reduction.
