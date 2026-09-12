# Project agreements

Read PRODUCT.md before planning or implementing. Its accepted product direction
supersedes the original open questions in README.md. Update the README as features
become available; distinguish implemented behavior from planned work.

Use Codex for every agent role, including planning, architecture, implementation,
review and recovery. Do not invoke Claude or fall back to another provider.

Work through the current Forge task only, preserving working increments and
existing review findings. Run meaningful checks appropriate to the change.
Forge manages commits and publishing; implementation agents must not independently
commit, push, reset review state, or modify .forge runtime files.

Keep commits short, focused and independently reviewable. Add no AI attribution
or Co-authored-by trailers. Retain the MIT license. Keep credentials, runtime
state, personal repository data and comment drafts out of Git.

The user authorized automatic pushes to origin for this development run. Publish
only after the required Forge review gates pass. Do not force-push.

Keep diagnostic tool output bounded: Forge rejects any single agent event over
1 MiB. When inspecting .forge history or agent records, select specific JSON
fields and bounded text excerpts, or use the project-targeted paginated Forge API.
Never dump or recursively search raw runtime records into tool output. Line-count
limits do not bound large JSONL records. Keep each result below 16 KiB; paginate
further when needed. Redirect verbose test output to a private temporary file,
then inspect its exit status and bounded relevant excerpts. Preserve full evidence
on disk and inspect all relevant findings without printing credentials.

Keep .agents/.gitkeep and .codex/.gitkeep as empty review-sandbox mount points.
Ignore all other contents of these directories, including provider runtime state.
