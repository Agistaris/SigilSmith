# SigilSmith workflow routes

Use this tracked index for contributor workflow. Root `AGENTS.md` is local and
ignored; never commit it. This index and its checks work without that file.

## Task routes

| Task | Start here |
| --- | --- |
| Rust implementation or runtime bug | The named `src/` file and relevant tests; [development run](../README.md#run-dev) when runtime evidence is needed |
| Local build or installation | [Build from source](../README.md#from-source) or [installation](INSTALL.md) |
| Packaging or release explicitly in scope | [Release checklist](RELEASE.md); its commands are procedures, not authorization to execute them |
| Documentation/routing only | Run `python3 -B scripts/test_workflow_docs.py` from the repository root; [the local check](../scripts/test_workflow_docs.py) also runs in docs-only CI |

Release, push, tag, upload, and publication require explicit user authorization.
Keep application data and unrelated work intact. The release checklist has no
maintained mod-site publishing runbook; establish those steps before publishing.

Open only the task's relevant documentation and source. Use the narrowest useful
verification; documentation checks do not run Rust builds, packaging, releases,
or application/data mutations. Reuse unchanged evidence and stop after the
required checks pass unless a change or new finding warrants more testing.

## Completion record

Keep this in the existing task record; concise user-facing prose may combine
fields. Use the governing R0/R1/R2 definitions without copying them here.
The checks protect discoverability and fields, not proof of agent compliance.

```text
Step: Current checklist item and completion state
Change: Files or behavior changed, or inspection finding
Checks: New/reused results and any runtime or release evidence still missing
Review: R0 rationale or R1/R2 result and finding dispositions
Blockers: Unresolved items or none
Next: Next action and any required owner checkpoint
```
