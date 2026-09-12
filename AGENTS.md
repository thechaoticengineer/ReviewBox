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
