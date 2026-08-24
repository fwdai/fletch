//! The stale-agent-image GC's listing format, parsing and selection rule —
//! *which* images to reclaim, never how to list or remove them (each runtime's
//! `cleanup` module supplies the listings and runs the `rmi`s).
//!
//! One rule: an image carrying the `fletch.agent` label
//! ([`images::AGENT_IMAGE_LABEL`](super::images::AGENT_IMAGE_LABEL)) that is not
//! one of the current expected tags is removed. Anything we can't attribute
//! survives — the under-reclaim bias the container sweep takes too.

use std::collections::HashSet;

use super::images::{image_repo, image_tag};
use super::ContainerProvider;

/// `images` line format for the GC listings. Both runtimes print `<none>` for a
/// dangling image's repository and tag.
pub(crate) const IMAGES_FORMAT: &str = "{{.ID}} {{.Repository}} {{.Tag}}";

/// One `images` row. A multi-tagged image yields one row per tag, which is what
/// the GC wants: it untags Fletch's name and leaves any other name alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImageRow {
    pub(crate) id: String,
    pub(crate) repo: String,
    pub(crate) tag: String,
}

impl ImageRow {
    fn untagged(&self) -> bool {
        self.repo == "<none>" || self.tag == "<none>"
    }

    /// Spelled exactly as the listing printed it — this is what `rmi` resolves.
    fn named(&self) -> String {
        format!("{}:{}", self.repo, self.tag)
    }

    /// The repo without podman's implicit `localhost/` prefix on locally built
    /// images. Identity under docker, which never prints one.
    pub(crate) fn compare_repo(&self) -> &str {
        self.repo.strip_prefix("localhost/").unwrap_or(&self.repo)
    }

    /// The spelling every namespace/tag comparison uses, so
    /// `localhost/fletch-agent` is recognized as ours while [`Self::named`]
    /// still names it verbatim for removal.
    fn compare_name(&self) -> String {
        format!("{}:{}", self.compare_repo(), self.tag)
    }

    /// The name for tagged rows (untag just ours), the id for dangling ones
    /// (their only handle).
    fn removal_ref(&self) -> String {
        if self.untagged() {
            self.id.clone()
        } else {
            self.named()
        }
    }
}

/// One [`IMAGES_FORMAT`] line → an [`ImageRow`]; `None` on malformed lines
/// (skipped — under-reclaim bias, same as the container sweep).
pub(crate) fn parse_images_line(line: &str) -> Option<ImageRow> {
    let mut parts = line.split_whitespace();
    let row = ImageRow {
        id: parts.next()?.to_string(),
        repo: parts.next()?.to_string(),
        tag: parts.next()?.to_string(),
    };
    Some(row)
}

/// The tags a launch could legitimately use right now; everything else Fletch
/// built is superseded by definition.
pub(crate) fn current_tags() -> HashSet<String> {
    ContainerProvider::ALL
        .iter()
        .map(|p| image_tag(*p))
        .collect()
}

/// The image repos Fletch owns today (one per live provider).
pub(crate) fn known_repos() -> HashSet<&'static str> {
    ContainerProvider::ALL
        .iter()
        .map(|p| image_repo(*p))
        .collect()
}

/// The GC's selection rule, pure: `labeled` holds every image carrying the
/// `fletch.agent` label, `legacy` the tagged contents of Fletch's own repos
/// (pre-label installs). Returns deduplicated `rmi` refs. Every set is passed in
/// rather than read from the constants so the rule is testable on fixed inputs
/// — and so a runtime with no pre-label history (podman) can pass an empty
/// `legacy_tags` and get no legacy arm at all.
///
/// The label is the authority, fenced both ways: a *labeled* image is a
/// candidate only inside our namespace (`known_repos` + `retired_repos`) and
/// only under a tag Fletch itself could have written — the content-addressed
/// shape or none at all — because a human-written tag is the human's call. An
/// *unlabeled* image carries no ownership proof, so it goes only on an exact
/// `legacy_tags` match. Current tags and the image override are always kept.
pub(crate) fn image_removal_refs(
    labeled: &[ImageRow],
    legacy: &[ImageRow],
    current_tags: &HashSet<String>,
    known_repos: &HashSet<&'static str>,
    retired_repos: &[&str],
    legacy_tags: &[&str],
    override_image: Option<&str>,
) -> Vec<String> {
    let override_image = override_image.map(str::trim).filter(|s| !s.is_empty());
    // Survives no matter which listing produced it. The override is matched in
    // every spelling a user might have typed it: exact `repo:tag`, bare repo
    // (both runtimes read `foo` as `foo:latest`), or image id.
    let protected = |row: &ImageRow| {
        if !row.untagged() && current_tags.contains(&row.compare_name()) {
            return true;
        }
        let Some(ov) = override_image else {
            return false;
        };
        // An override may be spelled as a `sha256:`-prefixed id or a
        // `repo@sha256:…` digest ref; normalize each before comparing.
        let ov_id = ov.strip_prefix("sha256:").unwrap_or(ov);
        let ov_name = ov.split_once("@sha256:").map_or(ov, |(repo, _)| repo);
        id_matches(ov_id, &row.id)
            || (!row.untagged()
                && (ov_name == row.named()
                    || ov_name == row.compare_name()
                    || (row.tag == "latest"
                        && (ov_name == row.repo || ov_name == row.compare_repo()))))
    };

    // Retired repos count: the label has already proven the image is ours, and
    // without them a retired provider's still-tagged images are unreclaimable
    // (the legacy arm skips them as labeled, this arm as unknown).
    let in_our_namespace = |row: &ImageRow| {
        let repo = row.compare_repo();
        known_repos.contains(repo) || retired_repos.contains(&repo)
    };

    let mut seen = HashSet::new();
    let mut refs = Vec::new();
    let mut push = |row: &ImageRow| {
        let r = row.removal_ref();
        if seen.insert(r.clone()) {
            refs.push(r);
        }
    };

    for row in labeled {
        if !row.untagged() && !in_our_namespace(row) {
            continue; // labeled but re-tagged under a user name: never touch
        }
        if !row.untagged() && !is_content_addressed_tag(&row.tag) {
            continue; // human-written tag in our repo: their tag, their call
        }
        if protected(row) {
            continue;
        }
        push(row);
    }
    for row in legacy {
        // Exact match only: tag *shape* is never an ownership signal for an
        // unlabeled image, so a user's own `fletch-agent:deadbeefcafe` survives.
        if row.untagged() || !legacy_tags.contains(&row.compare_name().as_str()) {
            continue;
        }
        if protected(row) {
            continue;
        }
        push(row);
    }
    refs
}

/// Prefix-tolerant image-id equality: `{{.ID}}` prints a 12-char truncation
/// while a user pastes the full 64, so either side may be the prefix. The
/// 12-char floor stops a stray short string from matching half the store.
fn id_matches(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (short, long) = if a.len() < b.len() { (a, b) } else { (b, a) };
    short.len() >= 12 && long.starts_with(short)
}

/// Whether a tag has the shape Fletch's content addressing writes — exactly
/// 12 lowercase hex chars (`images::tag_for`'s `sha256[..12]`). Anything else
/// in a Fletch repo was written by a human and is never a removal candidate.
pub(crate) fn is_content_addressed_tag(tag: &str) -> bool {
    tag.len() == 12
        && tag
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Image repos Fletch used to own — bare repo names, never `repo:tag`, since
/// these images are labeled and the labeled arm's safety rules still apply.
///
/// **Add an entry whenever a [`ContainerProvider`] variant is removed.** Without
/// one, every still-tagged image under that repo is stranded on every user's
/// disk forever at ~0.5-1GB each: the labeled arm skips it (repo unknown) and
/// the legacy arm skips it (it's labeled). Empty until the first retirement.
pub(crate) const RETIRED_REPOS: &[&str] = &[];

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, repo: &str, tag: &str) -> ImageRow {
        ImageRow {
            id: id.into(),
            repo: repo.into(),
            tag: tag.into(),
        }
    }

    #[test]
    fn images_line_parsing() {
        assert_eq!(
            parse_images_line("abc123def456 fletch-agent 0011aabbccdd"),
            Some(row("abc123def456", "fletch-agent", "0011aabbccdd")),
        );
        assert_eq!(
            parse_images_line("abc123def456 <none> <none>"),
            Some(row("abc123def456", "<none>", "<none>")),
        );
        assert_eq!(parse_images_line("abc123def456 fletch-agent"), None);
        assert_eq!(parse_images_line(""), None);
    }

    #[test]
    fn image_gc_selection() {
        let current_tags: HashSet<String> = ["fletch-agent:cafe00000000".to_string()].into();
        let known_repos: HashSet<&'static str> = ["fletch-agent", "fletch-agent-codex"].into();
        let legacy_tags: &[&str] = &["fletch-agent-codex:fa189de85caf"];

        let labeled = vec![
            // Superseded hash: removed.
            row("aaa", "fletch-agent", "0dab1e000000"),
            // Current tag: kept.
            row("bbb", "fletch-agent", "cafe00000000"),
            // Dangling rebuild leftover: removed by id.
            row("ccc", "<none>", "<none>"),
            // Re-tagged under a user's name: kept.
            row("ddd", "mybackup", "keep"),
            // The override, hypothetically labeled: kept.
            row("eee", "ghcr.io/me/custom", "1"),
            // Human-written tag inside our repo: kept.
            row("hhh", "fletch-agent", "backup"),
        ];
        let legacy = vec![
            // Genuine pre-label image, exact legacy-list match: removed.
            row("fff", "fletch-agent-codex", "fa189de85caf"),
            // Current tag, also in the repo-scoped listing: kept.
            row("bbb", "fletch-agent", "cafe00000000"),
            // Unlabeled non-fletch row, in case a listing misbehaves: kept.
            row("ggg", "someones-image", "latest"),
            // A user's own image in our namespace, human tag: kept.
            row("iii", "fletch-agent", "backup"),
            // Same, under a hex-shaped tag — shape is not ownership: kept.
            row("jjj", "fletch-agent", "deadbeefcafe"),
        ];

        let refs = image_removal_refs(
            &labeled,
            &legacy,
            &current_tags,
            &known_repos,
            &[],
            legacy_tags,
            Some("ghcr.io/me/custom:1"),
        );
        assert_eq!(
            refs,
            vec![
                "fletch-agent:0dab1e000000".to_string(),
                "ccc".to_string(),
                "fletch-agent-codex:fa189de85caf".to_string(),
            ],
        );

        // An empty legacy list (what podman passes) drops the legacy arm.
        let refs = image_removal_refs(
            &labeled,
            &legacy,
            &current_tags,
            &known_repos,
            &[],
            &[],
            Some("ghcr.io/me/custom:1"),
        );
        assert_eq!(
            refs,
            vec!["fletch-agent:0dab1e000000".to_string(), "ccc".to_string()],
        );
    }

    #[test]
    fn content_addressed_tag_shape() {
        assert!(is_content_addressed_tag("0123abcdef01"));
        assert!(is_content_addressed_tag("000000000000"));
        // Human-shaped, wrong length, uppercase: all kept.
        assert!(!is_content_addressed_tag("backup"));
        assert!(!is_content_addressed_tag("latest"));
        assert!(!is_content_addressed_tag("0123ABCDEF01"));
        assert!(!is_content_addressed_tag("0123abcdef0"));
        assert!(!is_content_addressed_tag("0123abcdef012"));
        assert!(!is_content_addressed_tag(""));
    }

    #[test]
    fn image_gc_override_spellings() {
        let current_tags = HashSet::new();
        let known_repos: HashSet<&'static str> = ["fletch-agent"].into();
        // Worst case: the override lives inside a Fletch repo under a
        // content-addressed-looking tag, past the tag-shape guard.
        let labeled = vec![
            row("aaa", "fletch-agent", "aaaaaaaaaaaa"),
            row("bbb", "fletch-agent", "bbbbbbbbbbbb"),
        ];

        // Exact repo:tag protects that row only.
        let refs = image_removal_refs(
            &labeled,
            &[],
            &current_tags,
            &known_repos,
            &[],
            &[],
            Some("fletch-agent:aaaaaaaaaaaa"),
        );
        assert_eq!(refs, vec!["fletch-agent:bbbbbbbbbbbb".to_string()]);

        // An id protects by id.
        let refs = image_removal_refs(
            &labeled,
            &[],
            &current_tags,
            &known_repos,
            &[],
            &[],
            Some("bbb"),
        );
        assert_eq!(refs, vec!["fletch-agent:aaaaaaaaaaaa".to_string()]);

        // A bare repo reads as `:latest`, which the tag-shape guard already
        // keeps — with or without the override present.
        let with_latest = vec![row("lll", "fletch-agent", "latest")];
        for ov in [Some("fletch-agent"), None] {
            assert!(image_removal_refs(
                &with_latest,
                &[],
                &current_tags,
                &known_repos,
                &[],
                &[],
                ov
            )
            .is_empty());
        }

        // Blank override protects nothing (same as None).
        let refs = image_removal_refs(
            &labeled,
            &[],
            &current_tags,
            &known_repos,
            &[],
            &[],
            Some("  "),
        );
        assert_eq!(refs.len(), 2);

        // Overlapping listings dedupe to one removal ref.
        let refs = image_removal_refs(
            &labeled,
            &labeled,
            &current_tags,
            &known_repos,
            &[],
            &[],
            None,
        );
        assert_eq!(refs.len(), 2);
    }

    /// Comparisons see through podman's `localhost/` prefix; the removal ref
    /// keeps it, since that is the name `podman rmi` resolves.
    #[test]
    fn image_gc_sees_through_podmans_localhost_prefix() {
        let current_tags: HashSet<String> = ["fletch-agent:cafe00000000".to_string()].into();
        let known_repos: HashSet<&'static str> = ["fletch-agent"].into();

        let labeled = vec![
            // Superseded: selected, named verbatim.
            row("aaa", "localhost/fletch-agent", "0dab1e000000"),
            // Current tag: protected.
            row("bbb", "localhost/fletch-agent", "cafe00000000"),
            // Outside our repos: untouched.
            row("ccc", "localhost/my-image", "0dab1e000000"),
        ];

        let refs = image_removal_refs(&labeled, &[], &current_tags, &known_repos, &[], &[], None);
        assert_eq!(
            refs,
            vec!["localhost/fletch-agent:0dab1e000000".to_string()],
        );
    }

    #[test]
    fn image_gc_override_id_and_digest_spellings() {
        let current_tags = HashSet::new();
        let known_repos: HashSet<&'static str> = ["fletch-agent"].into();
        let full_id = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let labeled = vec![
            row("0123456789ab", "fletch-agent", "aaaaaaaaaaaa"),
            row("bbbbbbbbbbbb", "fletch-agent", "bbbbbbbbbbbb"),
        ];
        let kept_bbb = vec!["fletch-agent:bbbbbbbbbbbb".to_string()];

        for ov in [full_id.to_string(), format!("sha256:{full_id}")] {
            let refs = image_removal_refs(
                &labeled,
                &[],
                &current_tags,
                &known_repos,
                &[],
                &[],
                Some(&ov),
            );
            assert_eq!(refs, kept_bbb, "id override spelling {ov} must protect");
        }

        // A digest ref: the `@sha256:…` half is stripped before matching.
        let refs = image_removal_refs(
            &labeled,
            &[],
            &current_tags,
            &known_repos,
            &[],
            &[],
            Some("fletch-agent:aaaaaaaaaaaa@sha256:0123456789abcdef"),
        );
        assert_eq!(refs, kept_bbb);

        // A short string is never an id prefix — 12 chars is the floor.
        assert!(id_matches("0123456789ab", full_id));
        assert!(id_matches(full_id, "0123456789ab"));
        assert!(!id_matches("0123456789a", full_id));
        assert!(id_matches("bbb", "bbb"), "exact equality still holds");
        assert!(!id_matches("bbb", "bbbbbbbbbbbb"));
    }

    /// A retired repo buys a row into the candidate set, not past the safety
    /// rules: the tag shape and the override still fence it.
    #[test]
    fn image_gc_reclaims_retired_provider_repos() {
        let current_tags: HashSet<String> = ["fletch-agent:cafe00000000".to_string()].into();
        let known_repos: HashSet<&'static str> = ["fletch-agent"].into();
        let retired: &[&str] = &["fletch-agent-pi"];

        let labeled = vec![
            // The stranding case: labeled, our old repo, our tag shape.
            row("aaa", "fletch-agent-pi", "abc123def456"),
            // Human-written tag in the retired repo: kept.
            row("bbb", "fletch-agent-pi", "backup"),
            // Neither live nor retired: never ours.
            row("ccc", "someones-image", "0dab1e000000"),
            // Retired repo, but it's the image override.
            row("ddd", "fletch-agent-pi", "0dab1e000000"),
            // Live repo, current tag.
            row("eee", "fletch-agent", "cafe00000000"),
        ];

        let refs = image_removal_refs(
            &labeled,
            &[],
            &current_tags,
            &known_repos,
            retired,
            &[],
            Some("fletch-agent-pi:0dab1e000000"),
        );
        assert_eq!(refs, vec!["fletch-agent-pi:abc123def456".to_string()]);

        // An empty retired list is a strict no-op.
        assert!(image_removal_refs(
            &labeled,
            &[],
            &current_tags,
            &known_repos,
            &[],
            &[],
            Some("fletch-agent-pi:0dab1e000000"),
        )
        .is_empty());
    }

    /// A listing's repo is compared whole, so a stray `repo:tag` entry in
    /// [`RETIRED_REPOS`] would silently match nothing.
    #[test]
    fn retired_repos_are_bare_fletch_repo_names() {
        let known = known_repos();
        for repo in RETIRED_REPOS {
            assert!(!repo.contains(':'), "retired entry must be a repo: {repo}");
            assert!(
                repo.starts_with("fletch-agent"),
                "retired repo isn't a fletch namespace: {repo}",
            );
            assert!(
                !known.contains(repo),
                "still-live repo listed as retired: {repo}",
            );
        }
    }

    /// Every current tag lives in a known repo, so a live image is never a
    /// candidate by namespace alone.
    #[test]
    fn expected_sets_cover_every_live_provider() {
        let tags = current_tags();
        let repos = known_repos();
        assert_eq!(tags.len(), ContainerProvider::ALL.len());
        for tag in &tags {
            let (repo, _) = tag.split_once(':').expect("image tags are repo:tag");
            assert!(repos.contains(repo), "current tag outside our repos: {tag}");
        }
    }
}
