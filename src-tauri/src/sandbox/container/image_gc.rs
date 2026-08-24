//! The stale-agent-image GC's listing format, parsing and selection rule —
//! everything about *which* images to reclaim, with nothing about how to list or
//! remove them.
//!
//! Both runtimes print their listings through the same Go-template format
//! ([`IMAGES_FORMAT`], which we choose), so the rows parse identically and the
//! selection rule below is shared verbatim. Each runtime's `cleanup` module
//! supplies the listings and runs the `rmi`s.
//!
//! One rule: an image carrying the `fletch.agent` label
//! ([`images::AGENT_IMAGE_LABEL`](super::images::AGENT_IMAGE_LABEL)) that is not
//! one of the current expected tags is removed. Anything we can't attribute
//! survives — the under-reclaim bias the container sweep takes too.

use std::collections::HashSet;

use super::images::{image_repo, image_tag};
use super::ContainerProvider;

/// `images` line format for the image GC listings. Untagged (dangling) images
/// print `<none>` for repository and tag under both runtimes.
pub(crate) const IMAGES_FORMAT: &str = "{{.ID}} {{.Repository}} {{.Tag}}";

/// One `images` row: a (repo:tag, image id) pair — the same image id appears in
/// multiple rows when it carries multiple tags, which is exactly what the GC
/// wants: it untags Fletch's name and leaves any other name alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ImageRow {
    pub(crate) id: String,
    pub(crate) repo: String,
    pub(crate) tag: String,
}

impl ImageRow {
    /// Whether this row is a dangling image (`<none>:<none>`) — removable only
    /// by id.
    fn untagged(&self) -> bool {
        self.repo == "<none>" || self.tag == "<none>"
    }

    /// The `repo:tag` name of a tagged row, spelled exactly as the listing
    /// printed it — this is what `rmi` has to resolve.
    fn named(&self) -> String {
        format!("{}:{}", self.repo, self.tag)
    }

    /// The repo without podman's implicit `localhost/` prefix for locally built
    /// images. Identity under docker, which never prints one.
    pub(crate) fn compare_repo(&self) -> &str {
        self.repo.strip_prefix("localhost/").unwrap_or(&self.repo)
    }

    /// [`Self::named`] under [`Self::compare_repo`] — the spelling every
    /// namespace/tag comparison uses, so `localhost/fletch-agent` is recognized
    /// as ours while removal still names it verbatim.
    fn compare_name(&self) -> String {
        format!("{}:{}", self.compare_repo(), self.tag)
    }

    /// What to hand `rmi`: the name for tagged rows (untag just ours), the id
    /// for dangling ones (the only handle they have).
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

/// The tags a launch could legitimately use right now — `image_tag(provider)`
/// across every container-supported provider. Everything else Fletch built is
/// superseded by definition.
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

/// The GC's selection rule, pure (inputs are pre-fetched listings, fixed in
/// tests). `labeled` holds every image carrying the `fletch.agent` label;
/// `legacy` holds the tagged contents of Fletch's own repos (pre-label
/// installs). Returns deduplicated `rmi` refs.
///
/// The label is the authority, with one belt-and-braces exception each way:
/// a *labeled* image tagged outside our namespace is kept (a user re-tagged
/// our image under their own name — their tag, their call), and an *unlabeled*
/// image is removed only on an exact `legacy_tags` match (the closed list of
/// tags pre-label Fletch actually shipped — shape or namespace alone is never
/// an ownership signal for unlabeled images). Current tags and the runtime's
/// image override are always kept.
///
/// "Our namespace" is `known_repos` (the live providers) plus `retired_repos`
/// ([`RETIRED_REPOS`] — providers we used to ship). Both are passed in rather
/// than read from the constants so the rule stays testable on fixed inputs, as
/// is `legacy_tags`: a runtime with no pre-label history (podman) passes an
/// empty list and gets no legacy arm at all.
///
/// Within Fletch's repos, a labeled image is a removal candidate only under a
/// tag Fletch itself could have written: the content-addressed shape (12
/// lowercase hex chars — see `images::tag_for`) or no tag at all (a dangling
/// rebuild predecessor). A human-shaped tag like `fletch-agent:backup` —
/// a user's `docker tag` of our image — is theirs to keep: a tag a human
/// wrote is the human's call. Retirement changes nothing about that: a
/// retired repo buys a row into the candidate set, not past the safety rules.
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
    // A row that must survive no matter which listing produced it. The
    // override comparison is defensive by design: match its exact `repo:tag`,
    // its bare-repo spelling (both runtimes read `foo` as `foo:latest`), or the
    // image id (all listings print the same short id).
    let protected = |row: &ImageRow| {
        if !row.untagged() && current_tags.contains(&row.compare_name()) {
            return true;
        }
        let Some(ov) = override_image else {
            return false;
        };
        // An override may be spelled as an id (`sha256:`-prefixed on docker) or
        // as a `repo@sha256:…` digest ref; normalize each before its comparison.
        let ov_id = ov.strip_prefix("sha256:").unwrap_or(ov);
        let ov_name = ov.split_once("@sha256:").map_or(ov, |(repo, _)| repo);
        id_matches(ov_id, &row.id)
            || (!row.untagged()
                && (ov_name == row.named()
                    || ov_name == row.compare_name()
                    || (row.tag == "latest"
                        && (ov_name == row.repo || ov_name == row.compare_repo()))))
    };

    // A repo Fletch owns now or used to own. Retired repos count because the
    // `fletch.agent` label has already proven the image is ours — omitting
    // them is exactly what stranded a retired provider's still-tagged images:
    // the legacy arm skips them (they're labeled) and this arm skipped them
    // (their repo was no longer known), so nothing could ever reclaim them.
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
        // Exact match only: an unlabeled image carries no ownership proof, so
        // the only safe removal signal is a tag we know Fletch shipped. A
        // user's own image in our namespace — even under a hex/git-SHA-shaped
        // tag like `fletch-agent:deadbeefcafe` — never matches.
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
/// while a user pastes the full 64-char id, so either side may be the prefix.
/// The shorter side must be at least 12 chars, or a stray short string would
/// match half the store.
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

/// Every image repo Fletch used to own — the closed list of namespaces that
/// are ours by history rather than by [`ContainerProvider::ALL`]. Bare repo
/// names (`fletch-agent-x`), never `repo:tag`: these images DO carry the
/// `fletch.agent` label, so ownership is already proven and the GC needs no
/// per-tag allowlist to act on them — the ordinary safety rules for labeled
/// rows (content-addressed tag shape, current tags, the image override) still
/// apply unchanged. Runtime-neutral: a retired provider's images are stranded
/// the same way in either store.
///
/// **Add an entry whenever a [`ContainerProvider`] variant is removed.**
/// Without one, every still-tagged image under that repo is stranded on every
/// user's disk forever, at ~0.5-1GB each: the labeled arm would skip it (repo
/// no longer known) and the legacy arm would skip it (it's labeled). Only
/// *untagged* rows survive a retirement today, since they never consult the
/// repo at all.
///
/// Empty until the first retirement — this is forward-safety, not a live bug,
/// and an empty list is behaviourally identical to not having one.
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
        // Dangling images print `<none>` placeholders.
        assert_eq!(
            parse_images_line("abc123def456 <none> <none>"),
            Some(row("abc123def456", "<none>", "<none>")),
        );
        assert_eq!(parse_images_line("abc123def456 fletch-agent"), None);
        assert_eq!(parse_images_line(""), None);
    }

    /// The GC's one rule plus its fences, on fixed listings: labeled + stale
    /// → remove; labeled + current → keep; labeled outside Fletch's repos
    /// (user re-tag) → keep; unlabeled non-fletch → keep; legacy fletch-repo
    /// stale → remove; the image override → keep in every spelling.
    #[test]
    fn image_gc_selection() {
        let current_tags: HashSet<String> = ["fletch-agent:cafe00000000".to_string()].into();
        let known_repos: HashSet<&'static str> = ["fletch-agent", "fletch-agent-codex"].into();
        let legacy_tags: &[&str] = &["fletch-agent-codex:fa189de85caf"];

        let labeled = vec![
            // Old hash under a Fletch repo: superseded by a Dockerfile revision.
            row("aaa", "fletch-agent", "0dab1e000000"),
            // The current tag: what launches use today.
            row("bbb", "fletch-agent", "cafe00000000"),
            // Untagged leftover of a TTL rebuild: removable only by id.
            row("ccc", "<none>", "<none>"),
            // Labeled but re-tagged under a user's name: their tag, kept.
            row("ddd", "mybackup", "keep"),
            // The override, hypothetically labeled (shouldn't happen): kept.
            row("eee", "ghcr.io/me/custom", "1"),
            // Labeled but human-tagged inside our repo (a `tag` of our
            // image): their tag, kept.
            row("hhh", "fletch-agent", "backup"),
        ];
        let legacy = vec![
            // Genuine pre-label image: exact legacy-list match, removable.
            row("fff", "fletch-agent-codex", "fa189de85caf"),
            // Current tag also shows up in the repo-scoped listing: kept.
            row("bbb", "fletch-agent", "cafe00000000"),
            // Selection must be safe even if a listing misbehaves: an
            // unlabeled non-fletch row is never removed.
            row("ggg", "someones-image", "latest"),
            // A user's own image built into our namespace: human tag, kept.
            row("iii", "fletch-agent", "backup"),
            // A user's own image under a hex/git-SHA-shaped tag in our
            // namespace: shape is not ownership — kept (exact-match only).
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

        // An empty legacy list — what a runtime with no pre-label history
        // passes — drops the legacy arm entirely and keeps the labeled verdict.
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

    /// Only tags Fletch's content addressing could have written count as
    /// removal candidates — exactly 12 lowercase hex chars.
    #[test]
    fn content_addressed_tag_shape() {
        assert!(is_content_addressed_tag("0123abcdef01"));
        assert!(is_content_addressed_tag("000000000000"));
        // Human-shaped tags, wrong length, uppercase: all kept.
        assert!(!is_content_addressed_tag("backup"));
        assert!(!is_content_addressed_tag("latest"));
        assert!(!is_content_addressed_tag("0123ABCDEF01"));
        assert!(!is_content_addressed_tag("0123abcdef0"));
        assert!(!is_content_addressed_tag("0123abcdef012"));
        assert!(!is_content_addressed_tag(""));
    }

    /// Override matching is defensive across spellings: exact `repo:tag`,
    /// bare repo (the implicit `:latest`), and image id all protect.
    #[test]
    fn image_gc_override_spellings() {
        let current_tags = HashSet::new();
        let known_repos: HashSet<&'static str> = ["fletch-agent"].into();
        // Hypothetical worst case: the user's override lives *inside* a
        // Fletch repo under a content-addressed-looking tag (anything
        // human-shaped is already kept by the tag-shape guard).
        let labeled = vec![
            row("aaa", "fletch-agent", "aaaaaaaaaaaa"),
            row("bbb", "fletch-agent", "bbbbbbbbbbbb"),
        ];

        // Exact repo:tag override protects that row only.
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

        // Id override protects by id.
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

        // A bare-repo override reads as `:latest` — and a `:latest` row in our
        // repo is kept by the tag-shape guard even before the override check,
        // with or without the override present.
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

    /// Podman prints locally built images as `localhost/<repo>`, so every
    /// namespace and tag comparison has to see through that prefix while the
    /// removal ref keeps it (that is the name `podman rmi` resolves).
    #[test]
    fn image_gc_sees_through_podmans_localhost_prefix() {
        let current_tags: HashSet<String> = ["fletch-agent:cafe00000000".to_string()].into();
        let known_repos: HashSet<&'static str> = ["fletch-agent"].into();

        let labeled = vec![
            // Superseded, under podman's spelling: selected, named verbatim.
            row("aaa", "localhost/fletch-agent", "0dab1e000000"),
            // The current tag under the same spelling: protected.
            row("bbb", "localhost/fletch-agent", "cafe00000000"),
            // A user's own `localhost/` image outside our repos: untouched.
            row("ccc", "localhost/my-image", "0dab1e000000"),
        ];

        let refs = image_removal_refs(&labeled, &[], &current_tags, &known_repos, &[], &[], None);
        assert_eq!(
            refs,
            vec!["localhost/fletch-agent:0dab1e000000".to_string()],
        );
    }

    /// An override the user pasted as a full id, a `sha256:`-prefixed id, or a
    /// `repo@sha256:…` digest ref must still protect its image.
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

        // A digest ref: the `@sha256:…` half is stripped before the name match.
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

    /// Retiring a provider must not strand its images. A labeled row under a
    /// retired repo is reclaimable, but only on the same terms as a live one:
    /// the content-addressed tag shape and the override still fence it, and a
    /// repo that is neither live nor retired is still untouchable.
    #[test]
    fn image_gc_reclaims_retired_provider_repos() {
        let current_tags: HashSet<String> = ["fletch-agent:cafe00000000".to_string()].into();
        let known_repos: HashSet<&'static str> = ["fletch-agent"].into();
        let retired: &[&str] = &["fletch-agent-pi"];

        let labeled = vec![
            // The stranding case: labeled, our old repo, our tag shape.
            row("aaa", "fletch-agent-pi", "abc123def456"),
            // Human-written tag in the retired repo: retirement is not a
            // licence to delete a tag a human wrote.
            row("bbb", "fletch-agent-pi", "backup"),
            // Never ours: not a live repo, not a retired one.
            row("ccc", "someones-image", "0dab1e000000"),
            // Retired repo, but it's the user's image override.
            row("ddd", "fletch-agent-pi", "0dab1e000000"),
            // Live repo, current tag: unaffected by any of this.
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

        // The shipped list is empty, and an empty list is a strict no-op:
        // every one of those rows is now outside our namespace but for the
        // live-repo one, which is the current tag.
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

    /// [`RETIRED_REPOS`] holds bare repo names, not `repo:tag` — it widens the
    /// namespace the labeled arm trusts, so a stray tag would silently match
    /// nothing (a listing's repo is compared whole). Entries must also be
    /// genuinely retired: a repo a live provider still owns belongs in
    /// [`ContainerProvider::ALL`]'s derivation, not here.
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

    /// The expected-tag / known-repo sets the GC derives from the provider
    /// list: one entry per live provider, and every current tag lives in a
    /// known repo (so a live image is never a candidate by namespace alone).
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
