//! Bundled starter `SKILL.md` documents for Orca Bots (13 canonical skills).
//!
//! Every skill slug matches `BUNDLED_SKILLS_13` (`orca_bots_and_designs_spec.md §1.10`)
//! and installs into `skills/<slug>/SKILL.md` inside a bot's home directory.

use serde::Serialize;
use std::collections::HashSet;

use crate::{BotError, BotErrorCode};

/// Maximum length of a valid bot skill slug.
pub const BOT_SKILL_SLUG_MAX_LEN: usize = 64;

/// Exact count of bundled starter skills.
pub const BUNDLED_SKILL_COUNT: usize = 13;

/// Validates a bot skill slug (`^[a-z0-9]+(?:-[a-z0-9]+)*$`, `1..=64` ASCII bytes).
pub fn is_valid_skill_slug(slug: &str) -> bool {
    if slug.is_empty() || slug.len() > BOT_SKILL_SLUG_MAX_LEN {
        return false;
    }
    if slug.starts_with('-') || slug.ends_with('-') || slug.contains("--") {
        return false;
    }
    slug.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Returns the canonical bot-home relative path `skills/<slug>/SKILL.md`.
pub fn skill_relative_path(slug: &str) -> Result<String, BotError> {
    if !is_valid_skill_slug(slug) {
        return Err(BotError::new(
            BotErrorCode::InvalidArgument,
            format!("Invalid skill slug '{slug}': expected lowercase kebab-case [a-z0-9-]."),
        ));
    }
    Ok(format!("skills/{slug}/SKILL.md"))
}

/// A bundled starter `SKILL.md` entry shipped with Orca.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BundledBotSkill {
    pub slug: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    pub skill_md: &'static str,
}

/// All 13 bundled starter `SKILL.md` documents in canonical catalog order.
pub const BUNDLED_BOT_SKILLS: [BundledBotSkill; BUNDLED_SKILL_COUNT] = [
    BundledBotSkill {
        slug: "pr-review-checklist",
        title: "Pull Request Review Checklist",
        summary: "Structured code review covering correctness, edge cases, tests, and risk.",
        skill_md: "\
# Pull Request Review Checklist

## When to use
Use when reviewing an open pull request, preparing pre-merge feedback, or auditing unreviewed work before handing off to an owner.

## Checklist
- [ ] Read the PR title, description, and linked issue to verify the change matches stated intent.
- [ ] Check boundary conditions: empty inputs, overflow/underflow, null/missing fields, and Unicode/UTF-8 slicing.
- [ ] Verify error paths propagate structured errors rather than swallowing failures or panicking.
- [ ] Inspect concurrency, locking, retry idempotency, and state-transition invariants.
- [ ] Confirm public API or schema changes maintain backward compatibility or include a migration.
- [ ] Verify unit and integration tests exercise both happy paths and failure/regression cases.
- [ ] Flag hardcoded credentials, unsafe shell execution, or unbounded memory/CPU loops.
- [ ] Summarize blocking issues separately from optional style nits with exact file and line citations.

## Guardrails
- Never approve or merge a pull request on behalf of a non-owner requester.
- Do not leave speculative comments without verifying the referenced code in the diff.
- Keep review summaries concise and actionable so the author can resolve all blockers in one pass.
",
    },
    BundledBotSkill {
        slug: "bug-reproduction",
        title: "Bug Reproduction",
        summary: "Isolate minimal deterministic reproduction steps, environment, and root cause.",
        skill_md: "\
# Bug Reproduction

## When to use
Use when triaging a reported bug, flaky test, or user-facing crash before writing a fix or assigning an engineering worker.

## Checklist
- [ ] Capture exact versions, OS/architecture, feature flags, and configuration inputs.
- [ ] Reduce the report to the smallest deterministic command, payload, or test case that fails.
- [ ] Record exact observed output (exit code, error code, stack trace) alongside expected behavior.
- [ ] Identify the first failing boundary (parser, state transition, storage, RPC, or UI layer).
- [ ] Check whether recent commits or dependency updates introduced the regression.
- [ ] Verify whether the bug reproduces consistently or depends on timing, locale, or cached state.
- [ ] Package the repro into an automated failing regression test proposal for the fixing worker.

## Guardrails
- Never paste raw user secrets, cookies, or tokens from bug reports into tickets or public comments.
- Distinguish verified facts from hypotheses and state clearly when a bug could not be reproduced.
- Do not modify production data or live user state while attempting reproduction.
",
    },
    BundledBotSkill {
        slug: "ci-failure-triage",
        title: "CI Failure Triage",
        summary: "Classify red CI runs into code regression, flake, infra outage, or dependency drift.",
        skill_md: "\
# CI Failure Triage

## When to use
Use when a pull request check, nightly workflow, or release pipeline fails and needs rapid diagnosis.

## Checklist
- [ ] Inspect the first failing job and extract the primary compiler, linter, or test assertion error.
- [ ] Compare failing tests against files modified in the PR to separate direct regressions from unrelated failures.
- [ ] Check recent runs on the default branch to identify pre-existing failures or known flakes.
- [ ] Detect environment issues: runner OOM, disk exhaustion, registry timeout, or expired credentials.
- [ ] Verify whether a lockfile, toolchain version, or cache key changed in the triggering commit.
- [ ] Recommend the exact next action: targeted fix, single retry for transient infra failure, or escalation.

## Guardrails
- Never silently rerun a failing check more than twice without diagnosing the underlying cause.
- Never disable tests, weaken assertions, or bypass lint gates to turn CI green.
- Include direct job names and failing test identifiers so an engineer can jump straight to the error.
",
    },
    BundledBotSkill {
        slug: "release-readiness",
        title: "Release Readiness",
        summary: "Pre-flight verification gate before tagging, deploying, or announcing a release.",
        skill_md: "\
# Release Readiness

## When to use
Use when preparing a release branch, validating a release candidate tag, or running a pre-deploy go/no-go check.

## Checklist
- [ ] Confirm all required CI checks, security scans, and platform builds pass on the exact release commit.
- [ ] Verify version numbers match across manifests, installers, lockfiles, and release notes.
- [ ] Check open P0/P1 blockers and ensure every cherry-picked fix is included and tested.
- [ ] Validate database migrations and wire protocol changes are backward-compatible during rollout.
- [ ] Verify rollback procedure, feature flags, and monitoring alerts are ready before deployment.
- [ ] Confirm release artifacts have valid checksums/signatures and publish targets are accessible.
- [ ] Obtain explicit owner sign-off before tagging or triggering production deployment.

## Guardrails
- Never push a release tag or trigger a production deploy without explicit owner confirmation.
- Treat any red required check or unresolved data-loss bug as a hard no-go.
- Record the exact commit SHA audited so post-cut commits cannot slip in unreviewed.
",
    },
    BundledBotSkill {
        slug: "release-notes-style",
        title: "Release Notes Style",
        summary: "Turn merged PRs and resolved issues into clear, user-focused release notes.",
        skill_md: "\
# Release Notes Style

## When to use
Use when drafting changelogs, GitHub release bodies, or customer-facing upgrade summaries from merged changes.

## Checklist
- [ ] Group entries by user impact: Highlights, New Features, Improvements, Bug Fixes, and Breaking Changes.
- [ ] Lead each bullet with the user-visible outcome rather than internal function or file names.
- [ ] Call out breaking changes, deprecations, and required migration steps prominently at the top.
- [ ] Deduplicate revert pairs, follow-up fixups, and internal refactors with no user-facing effect.
- [ ] Include PR or issue references consistently and verify every mentioned feature actually shipped in the tag.
- [ ] Proofread for concise active voice, accurate product terminology, and clean Markdown formatting.

## Guardrails
- Never invent features or claim fixes that are not verified in the release commit range.
- Exclude sensitive security exploit details until coordinated disclosure is approved.
- Never publish release notes externally without owner review.
",
    },
    BundledBotSkill {
        slug: "dependency-upgrade",
        title: "Dependency Upgrade",
        summary: "Evaluate dependency updates for security advisories, breaking API changes, and lockfile hygiene.",
        skill_md: "\
# Dependency Upgrade

## When to use
Use when auditing outdated dependencies, responding to security advisories, or preparing targeted package upgrades.

## Checklist
- [ ] Identify exact current version, target version, and whether the bump is patch, minor, or major.
- [ ] Review upstream changelogs, advisories (CVE/GHSA), and license changes for direct and transitive crates.
- [ ] Check MSRV, platform target compatibility, and feature-flag defaults before upgrading.
- [ ] Isolate security patches from unrelated major upgrades so regressions are easy to bisect.
- [ ] Verify lockfile diff is minimal and no unintended duplicate major versions were pulled in.
- [ ] Run full build, lint, and test verification after updating and document any behavioral changes.

## Guardrails
- Never bulk-upgrade unrelated major dependencies in a single unreviewed PR.
- Never ignore yanked packages or unresolved critical security advisories.
- Keep offline/reproducible build invariants intact and verify lockfile integrity.
",
    },
    BundledBotSkill {
        slug: "security-checklist",
        title: "Security Checklist",
        summary: "Audit code, configs, and bot artifacts for credential leaks, injection, and privilege escalation.",
        skill_md: "\
# Security Checklist

## When to use
Use when reviewing authentication, cryptography, parsers, webhooks, tool relays, external connectors, or untrusted input handling.

## Checklist
- [ ] Confirm zero secrets, API keys, PEM blocks, or credentials appear in code, logs, fixtures, or bot home files.
- [ ] Verify all file paths reject traversal (`..`), absolute paths, symlinks, and control characters.
- [ ] Check external inputs enforce hard size, count, and complexity bounds to prevent DoS or ReDoS.
- [ ] Verify authentication, authorization, and host/origin checks fail closed on missing or malformed claims.
- [ ] Audit cryptographic code for domain separation, authenticated encryption (AEAD), non-zero DH secrets, and key zeroization.
- [ ] Ensure untrusted external text (Slack/GitHub/webhooks/imports) is fenced as data, never executed as instructions.
- [ ] Check subprocess invocations avoid shell interpolation (`sh -c`) and strip inherited sensitive environment variables.

## Guardrails
- Never bypass or weaken a security check to unblock a feature or test.
- Never log, echo, or commit live tokens, webhook secrets, or private keys during triage.
- Escalate confirmed vulnerabilities immediately to the owner via private channel rather than public issues.
",
    },
    BundledBotSkill {
        slug: "issue-triage",
        title: "Issue Triage",
        summary: "Classify incoming issues by severity, area, duplicate status, and missing reproduction details.",
        skill_md: "\
# Issue Triage

## When to use
Use when reviewing newly opened GitHub issues, Linear tickets, or escalated support reports.

## Checklist
- [ ] Search existing open and recently closed issues to detect duplicates or related regressions.
- [ ] Categorize the report accurately: bug, feature request, documentation gap, or support question.
- [ ] Assign severity and priority based on blast radius, data-loss risk, and availability of a workaround.
- [ ] Verify required context is present (version, steps to reproduce, expected vs actual outcome, logs).
- [ ] Route to the appropriate component label or owner and link related PRs or incidents.
- [ ] Ask one clear, concise follow-up question if critical reproduction data is missing.

## Guardrails
- Never close an active user bug as duplicate without linking the exact canonical tracking issue.
- Do not promise shipping dates or roadmap commitments without owner confirmation.
- Strip personal identifiers or secrets if summarizing private reports into shared issue trackers.
",
    },
    BundledBotSkill {
        slug: "feedback-themes",
        title: "Feedback Themes",
        summary: "Cluster qualitative customer and teammate feedback into evidence-backed themes.",
        skill_md: "\
# Feedback Themes

## When to use
Use when synthesizing Slack threads, support emails, user interviews, or issue comments into actionable product insights.

## Checklist
- [ ] Collect raw feedback entries with source link, date, customer segment, and exact user quote or symptom.
- [ ] Group recurring pain points into crisp theme clusters and count distinct reporters per theme.
- [ ] Separate observed user friction from proposed solutions so engineering can evaluate root causes.
- [ ] Highlight severity, frequency trend (rising, steady, resolved), and representative anonymized examples.
- [ ] Cross-reference each top theme with existing backlog issues or shipped fixes.
- [ ] Propose 2 to 4 concrete follow-up actions ranked by user impact and effort.

## Guardrails
- Never extrapolate a single anecdote into a broad trend without stating the exact sample count.
- Redact customer emails, phone numbers, and account secrets before sharing digests broadly.
- Preserve dissenting or edge-case feedback separately so minority workflows are not erased.
",
    },
    BundledBotSkill {
        slug: "status-brief",
        title: "Status Brief",
        summary: "Produce crisp, scannable progress updates covering shipped work, active blockers, and next decisions.",
        skill_md: "\
# Status Brief

## When to use
Use when compiling daily standups, weekly leadership digests, project health briefs, or pre-meeting summaries.

## Checklist
- [ ] Lead with a one-sentence overall status (On Track, At Risk, Blocked) backed by concrete evidence.
- [ ] List completed deliverables with links to merged PRs, closed items, or verified milestones.
- [ ] Surface active work in progress with current owner and expected checkpoint.
- [ ] Call out blockers, overdue reviews, and explicit decisions needed from the owner.
- [ ] Highlight meaningful metric shifts, incident carry-overs, or upcoming deadlines in the next 7 days.
- [ ] Keep formatting tight: bullet lists, short bold labels, and zero filler prose.

## Guardrails
- Never report unverified or in-flight work as completed.
- Flag stale or silent dependencies explicitly instead of assuming they are progressing.
- Omit noisy low-signal commits when summarizing for cross-functional audiences.
",
    },
    BundledBotSkill {
        slug: "spec-format",
        title: "Technical Spec Format",
        summary: "Draft clear architecture and feature specs with explicit invariants, edge cases, and non-goals.",
        skill_md: "\
# Technical Spec Format

## When to use
Use when drafting a new feature design, API/schema proposal, or migration plan before implementation begins.

## Checklist
- [ ] State the problem, target users, concrete goals, and explicit non-goals up front.
- [ ] Define data models, wire contracts, error codes, and backward-compatibility constraints precisely.
- [ ] Walk through happy paths, failure modes, retry semantics, and concurrency boundaries.
- [ ] Document security considerations: authentication, authorization, input bounds, and secret isolation.
- [ ] Specify observability, rollback plan, and measurable acceptance criteria.
- [ ] Call out open questions and recommended defaults so reviewers can decide quickly.

## Guardrails
- Never hide unresolved design trade-offs behind vague wording or placeholder sections.
- Keep diagrams and tables aligned with actual repository module names and interfaces.
- Avoid prescribing implementation details that constrain safe refactors without reason.
",
    },
    BundledBotSkill {
        slug: "incident-timeline",
        title: "Incident Timeline",
        summary: "Reconstruct chronological incident facts, mitigations, customer impact, and follow-up actions.",
        skill_md: "\
# Incident Timeline

## When to use
Use during or immediately after a service degradation, outage, or Sev-1/Sev-2 alert to maintain an accurate timeline and handoff record.

## Checklist
- [ ] Record all timestamps in UTC (ISO-8601) with exact source links to alerts, deploys, and Slack threads.
- [ ] Capture detection time, mitigation time, full recovery time, and affected user/region scope.
- [ ] Log every mitigation attempt, configuration toggle, and rollback with operator name and outcome.
- [ ] Distinguish confirmed root causes from working hypotheses still under investigation.
- [ ] List active handoff owners, open alerts, and monitoring queries to watch during the next shift.
- [ ] Extract concrete blameless postmortem action items with clear owners and priority.

## Guardrails
- Never edit or erase earlier timeline entries; append corrections with fresh timestamps instead.
- Never paste production credentials, private customer payloads, or raw PII into incident notes.
- Do not declare an incident resolved until error rates and latency return to baseline and stay stable.
",
    },
    BundledBotSkill {
        slug: "workspace-hygiene",
        title: "Workspace Hygiene",
        summary: "Audit stale git worktrees, merged branches, build caches, and orphaned temporary artifacts.",
        skill_md: "\
# Workspace Hygiene

## When to use
Use when reclaiming disk space, auditing merged feature worktrees, or cleaning up stale local branches and build artifacts.

## Checklist
- [ ] List all local worktrees and branches alongside merge status, dirty file count, and last commit timestamp.
- [ ] Identify merged branches and abandoned worktrees that have zero uncommitted or unpushed changes.
- [ ] Flag dirty or unpushed worktrees separately so no in-progress work is accidentally discarded.
- [ ] Measure build cache and temporary directory sizes and identify safe reclaimable targets.
- [ ] Present a dry-run cleanup plan with estimated disk savings before executing any destructive command.
- [ ] Verify the primary checkout remains clean and runnable after cleanup completes.

## Guardrails
- Never delete a branch or worktree containing uncommitted changes or unpushed commits without explicit owner approval.
- Never run `git reset --hard` or force-remove directories outside verified managed workspace roots.
- Always confirm cleanup commands against the exact repository path before execution.
",
    },
];

/// Looks up one of the 13 bundled starter skills by its exact slug.
pub fn find_bundled_skill(slug: &str) -> Option<&'static BundledBotSkill> {
    BUNDLED_BOT_SKILLS.iter().find(|skill| skill.slug == slug)
}

/// Returns all 13 canonical bundled skill slugs in catalog order.
pub const fn bundled_skill_slugs() -> [&'static str; BUNDLED_SKILL_COUNT] {
    [
        BUNDLED_BOT_SKILLS[0].slug,
        BUNDLED_BOT_SKILLS[1].slug,
        BUNDLED_BOT_SKILLS[2].slug,
        BUNDLED_BOT_SKILLS[3].slug,
        BUNDLED_BOT_SKILLS[4].slug,
        BUNDLED_BOT_SKILLS[5].slug,
        BUNDLED_BOT_SKILLS[6].slug,
        BUNDLED_BOT_SKILLS[7].slug,
        BUNDLED_BOT_SKILLS[8].slug,
        BUNDLED_BOT_SKILLS[9].slug,
        BUNDLED_BOT_SKILLS[10].slug,
        BUNDLED_BOT_SKILLS[11].slug,
        BUNDLED_BOT_SKILLS[12].slug,
    ]
}

/// Validates structural integrity of the bundled starter skill catalog.
pub fn validate_bundled_skills() -> Result<(), BotError> {
    let mut seen = HashSet::with_capacity(BUNDLED_BOT_SKILLS.len());
    for skill in &BUNDLED_BOT_SKILLS {
        if !is_valid_skill_slug(skill.slug) {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Bundled skill has invalid slug '{}'.", skill.slug),
            ));
        }
        if !seen.insert(skill.slug) {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Duplicate bundled skill slug '{}'.", skill.slug),
            ));
        }
        if skill.title.trim().is_empty() || skill.summary.trim().is_empty() {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!("Bundled skill '{}' has empty title or summary.", skill.slug),
            ));
        }
        let md = skill.skill_md;
        if !md.starts_with("# ")
            || !md.contains("\n## When to use\n")
            || !md.contains("\n## Checklist\n")
            || !md.contains("\n## Guardrails\n")
            || !md.contains("\n- [ ] ")
        {
            return Err(BotError::new(
                BotErrorCode::InvalidArgument,
                format!(
                    "Bundled skill '{}' is missing required Markdown checklist sections.",
                    skill.slug
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_skills_exact_13_catalog_and_checklists() {
        let expected = [
            "pr-review-checklist",
            "bug-reproduction",
            "ci-failure-triage",
            "release-readiness",
            "release-notes-style",
            "dependency-upgrade",
            "security-checklist",
            "issue-triage",
            "feedback-themes",
            "status-brief",
            "spec-format",
            "incident-timeline",
            "workspace-hygiene",
        ];
        assert_eq!(BUNDLED_BOT_SKILLS.len(), 13);
        assert_eq!(bundled_skill_slugs(), expected);
        assert!(validate_bundled_skills().is_ok());

        for slug in expected {
            let skill = find_bundled_skill(slug).expect("bundled skill exists");
            assert_eq!(skill.slug, slug);
            assert!(skill.skill_md.len() > 300);
            let checklist_count = skill.skill_md.matches("\n- [ ] ").count();
            assert!(checklist_count >= 5, "skill {slug} has too few checklist items");
            assert_eq!(
                skill_relative_path(slug).expect("valid skill path"),
                format!("skills/{slug}/SKILL.md")
            );
        }
        assert!(find_bundled_skill("nonexistent-skill").is_none());
    }

    #[test]
    fn skill_slug_validation_rejects_traversal_and_bad_chars() {
        assert!(is_valid_skill_slug("a"));
        assert!(is_valid_skill_slug("pr-review-checklist"));
        assert!(is_valid_skill_slug("v2-ci-triage"));
        assert!(!is_valid_skill_slug(""));
        assert!(!is_valid_skill_slug("-leading"));
        assert!(!is_valid_skill_slug("trailing-"));
        assert!(!is_valid_skill_slug("double--dash"));
        assert!(!is_valid_skill_slug("UpperCase"));
        assert!(!is_valid_skill_slug("../escape"));
        assert!(!is_valid_skill_slug("a/b"));
        assert!(!is_valid_skill_slug("a_b"));
        assert!(!is_valid_skill_slug(&"a".repeat(BOT_SKILL_SLUG_MAX_LEN + 1)));
        assert_eq!(
            skill_relative_path("../etc").expect_err("reject traversal").code,
            BotErrorCode::InvalidArgument
        );
    }
}
