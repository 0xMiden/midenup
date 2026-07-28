//! A canonical digest over the material inputs of an installation.
//!
//! # What this is not
//!
//! The key is **diagnostic and cache-input metadata**. It names nothing on disk: publication
//! directories are identified by an opaque, randomly generated id. Two equal keys do **not** imply
//! two equal directory trees, and a matching key never authorizes skipping work or reusing another
//! publication's content. Byte-level verification is explicitly out of scope, so any claim the key
//! made about content would be unfounded.
//!
//! What it is good for: telling whether the *inputs* to an installation changed, which is exactly
//! what update planning needs when deciding whether a component must be reinstalled.
//!
//! # Canonicalization
//!
//! Fields are written in a fixed order with explicit length prefixes, so no concatenation of
//! values can be confused for a different set of values. Collections are sorted by a declared key
//! so input ordering cannot change the result. Absent and empty are encoded distinctly -- a
//! component with no features must not hash the same as one whose features field was omitted.
//!
//! The `pk1:` prefix versions the algorithm. When destination policy or the input set changes, the
//! prefix changes too, so an old key compares as *unknown* rather than as *changed* -- the
//! difference matters, because "unknown" means reinstall while "changed" would imply we understood
//! what moved.

use std::fmt;

use sha2::Digest as _;

/// A canonical digest over an installation's material inputs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlanKey(String);

impl PlanKey {
    /// The algorithm version prefix. Bump when the input set or encoding changes.
    pub const PREFIX: &'static str = "pk1:";
}

impl fmt::Display for PlanKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Field tags. Values are part of the encoding, so they must never be renumbered -- only appended
/// to, and only alongside a `PREFIX` bump.
mod tag {
    pub const TARGET: u8 = 1;
    pub const COMPONENT: u8 = 2;
    pub const AUTHORITY: u8 = 3;
    pub const KIND: u8 = 4;
    pub const METHOD: u8 = 5;
    pub const ARTIFACT_ID: u8 = 6;
    pub const ARTIFACT_URI: u8 = 7;
    pub const DESTINATION: u8 = 8;
    pub const MODE: u8 = 9;
    pub const CRATE_NAME: u8 = 10;
    pub const FEATURE: u8 = 11;
    pub const RUSTUP_CHANNEL: u8 = 12;
    pub const SYMLINK: u8 = 13;
}

/// A canonical byte encoder.
#[derive(Default)]
struct Encoder {
    buffer: Vec<u8>,
}

impl Encoder {
    /// Writes a present value: tag, presence marker, length, bytes.
    fn field(&mut self, tag: u8, bytes: &[u8]) {
        self.buffer.push(tag);
        self.buffer.push(1);
        self.buffer.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        self.buffer.extend_from_slice(bytes);
    }

    /// Writes an optional value. Absent encodes distinctly from present-and-empty.
    fn optional(&mut self, tag: u8, value: Option<&str>) {
        match value {
            Some(value) => self.field(tag, value.as_bytes()),
            None => {
                self.buffer.push(tag);
                self.buffer.push(0);
            },
        }
    }

    fn text(&mut self, tag: u8, value: &str) {
        self.field(tag, value.as_bytes());
    }

    fn number(&mut self, tag: u8, value: u64) {
        self.field(tag, &value.to_le_bytes());
    }

    fn finish(self) -> PlanKey {
        let digest = sha2::Sha256::digest(&self.buffer);
        let mut out = String::with_capacity(PlanKey::PREFIX.len() + 64);
        out.push_str(PlanKey::PREFIX);
        for byte in digest {
            use fmt::Write;
            write!(&mut out, "{byte:02x}").expect("writing to a String cannot fail");
        }
        PlanKey(out)
    }
}

/// One component's contribution to the key.
///
/// Every field here is *material*: changing it changes a byte on disk. Selection, profiles,
/// aliases, call formats, subcommands, `initialization` and the channel alias are deliberately
/// absent -- they are resolved at dispatch time from local state and never materialize as files.
/// `opt/` symlinks *are* files, so the symlink layout is included.
#[derive(Debug, Clone, Default)]
pub struct ComponentInputs {
    pub name: String,
    /// Fully pinned: a git branch must already be resolved to a commit.
    pub authority: String,
    pub kind: String,
    pub installation_method: String,
    /// `(artifact id, resolved uri)`, in any order.
    pub artifacts: Vec<(String, String)>,
    /// `(exact destination path, file mode)`, in any order.
    pub destinations: Vec<(String, u32)>,
    pub crate_name: Option<String>,
    pub features: Option<Vec<String>>,
    pub rustup_channel: Option<String>,
    /// `(symlink name, target binary)`, in any order.
    pub symlinks: Vec<(String, String)>,
}

/// Everything a plan key is computed over.
#[derive(Debug, Clone, Default)]
pub struct KeyInputs {
    pub target: String,
    pub components: Vec<ComponentInputs>,
}

/// Computes the canonical key for `inputs`.
pub fn compute(inputs: &KeyInputs) -> PlanKey {
    let mut encoder = Encoder::default();
    encoder.text(tag::TARGET, &inputs.target);

    // Sorted so that input ordering cannot change the key.
    let mut components: Vec<&ComponentInputs> = inputs.components.iter().collect();
    components.sort_by(|a, b| a.name.cmp(&b.name));

    for component in components {
        encoder.text(tag::COMPONENT, &component.name);
        encoder.text(tag::AUTHORITY, &component.authority);
        encoder.text(tag::KIND, &component.kind);
        encoder.text(tag::METHOD, &component.installation_method);

        let mut artifacts = component.artifacts.clone();
        artifacts.sort();
        for (id, uri) in artifacts.iter() {
            encoder.text(tag::ARTIFACT_ID, id);
            encoder.text(tag::ARTIFACT_URI, uri);
        }

        let mut destinations = component.destinations.clone();
        destinations.sort();
        for (path, mode) in destinations.iter() {
            encoder.text(tag::DESTINATION, path);
            encoder.number(tag::MODE, *mode as u64);
        }

        encoder.optional(tag::CRATE_NAME, component.crate_name.as_deref());

        match component.features.as_ref() {
            Some(features) => {
                let mut features = features.clone();
                features.sort();
                // A present-but-empty list still emits its presence marker via `optional`, so it
                // cannot collide with an absent one.
                encoder.optional(tag::FEATURE, Some(""));
                for feature in features.iter() {
                    encoder.text(tag::FEATURE, feature);
                }
            },
            None => encoder.optional(tag::FEATURE, None),
        }

        encoder.optional(tag::RUSTUP_CHANNEL, component.rustup_channel.as_deref());

        let mut symlinks = component.symlinks.clone();
        symlinks.sort();
        for (name, target) in symlinks.iter() {
            encoder.text(tag::SYMLINK, name);
            encoder.text(tag::SYMLINK, target);
        }
    }

    encoder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn component(name: &str) -> ComponentInputs {
        ComponentInputs {
            name: name.to_string(),
            authority: "registry:0.15.0".to_string(),
            kind: "executable".to_string(),
            installation_method: "prebuilt".to_string(),
            artifacts: vec![("miden-vm".to_string(), "https://example.invalid/vm".to_string())],
            destinations: vec![("/s/bin/miden-vm".to_string(), 0o755)],
            crate_name: Some("miden-vm".to_string()),
            features: Some(vec!["std".to_string()]),
            rustup_channel: None,
            symlinks: vec![("miden vm".to_string(), "miden-vm".to_string())],
        }
    }

    fn inputs() -> KeyInputs {
        KeyInputs {
            target: "aarch64-apple-darwin".to_string(),
            components: vec![component("vm"), component("core"), component("client")],
        }
    }

    #[test]
    fn key_carries_an_algorithm_version_prefix() {
        assert!(compute(&inputs()).to_string().starts_with("pk1:"));
    }

    #[test]
    fn key_is_stable_under_input_reordering() {
        let mut reordered = inputs();
        reordered.components.reverse();
        assert_eq!(compute(&inputs()), compute(&reordered));

        // A rotation, not just a reversal, so the test does not accidentally depend on symmetry.
        let mut rotated = inputs();
        rotated.components.rotate_left(1);
        assert_eq!(compute(&inputs()), compute(&rotated));
    }

    #[test]
    fn collection_ordering_within_a_component_does_not_matter() {
        let mut a = inputs();
        a.components[0].symlinks = vec![("x".into(), "1".into()), ("y".into(), "2".into())];
        a.components[0].features = Some(vec!["b".into(), "a".into()]);

        let mut b = inputs();
        b.components[0].symlinks = vec![("y".into(), "2".into()), ("x".into(), "1".into())];
        b.components[0].features = Some(vec!["a".into(), "b".into()]);

        assert_eq!(compute(&a), compute(&b));
    }

    #[test]
    fn key_changes_for_every_material_input() {
        let base = inputs();
        type Mutation = (&'static str, fn(&mut KeyInputs));
        let mutations: Vec<Mutation> = vec![
            ("target", |i| i.target = "x86_64-unknown-linux-gnu".into()),
            ("component name", |i| i.components[0].name = "renamed".into()),
            ("authority", |i| i.components[0].authority = "registry:0.16.0".into()),
            ("kind", |i| i.components[0].kind = "package".into()),
            ("method", |i| i.components[0].installation_method = "cargo".into()),
            ("artifact id", |i| i.components[0].artifacts[0].0 = "other".into()),
            ("artifact uri", |i| i.components[0].artifacts[0].1 = "https://other".into()),
            ("destination", |i| i.components[0].destinations[0].0 = "/s/bin/other".into()),
            ("mode", |i| i.components[0].destinations[0].1 = 0o644),
            ("crate name", |i| i.components[0].crate_name = Some("other".into())),
            ("features", |i| i.components[0].features = Some(vec!["other".into()])),
            ("rustup channel", |i| i.components[0].rustup_channel = Some("nightly".into())),
            ("symlink name", |i| i.components[0].symlinks[0].0 = "miden other".into()),
            ("symlink target", |i| i.components[0].symlinks[0].1 = "other-bin".into()),
            ("component added", |i| i.components.push(component("extra"))),
            ("component removed", |i| {
                i.components.pop();
            }),
        ];

        for (label, mutate) in mutations {
            let mut mutated = base.clone();
            mutate(&mut mutated);
            assert_ne!(compute(&base), compute(&mutated), "key must change for {label}");
        }
    }

    /// Absent and present-but-empty must not collide: a component that declares no features and
    /// one whose features list is empty are different inputs.
    #[test]
    fn absent_and_empty_encode_differently() {
        let mut absent = inputs();
        absent.components[0].features = None;

        let mut empty = inputs();
        empty.components[0].features = Some(vec![]);

        assert_ne!(compute(&absent), compute(&empty));
    }

    #[test]
    fn absent_and_empty_optionals_encode_differently() {
        let mut absent = inputs();
        absent.components[0].rustup_channel = None;

        let mut empty = inputs();
        empty.components[0].rustup_channel = Some(String::new());

        assert_ne!(compute(&absent), compute(&empty));
    }

    /// Length prefixes mean adjacent fields cannot be confused for one another.
    #[test]
    fn field_boundaries_are_unambiguous() {
        let mut a = inputs();
        a.components[0].name = "ab".into();
        a.components[0].authority = "c".into();

        let mut b = inputs();
        b.components[0].name = "a".into();
        b.components[0].authority = "bc".into();

        assert_ne!(compute(&a), compute(&b));
    }

    #[test]
    fn the_key_is_deterministic_across_calls() {
        assert_eq!(compute(&inputs()), compute(&inputs()));
    }
}
