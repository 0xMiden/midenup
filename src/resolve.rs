//! Expanding a persisted selection into an ordered, installable component set.
//!
//! This is the single resolver. Everything that needs to know "which components does this
//! selection mean" goes through [resolve] -- direct installs, toolchain-file activation, updates,
//! and plan construction alike.
//!
//! It replaces two overlapping mechanisms that disagreed with each other: `Channel::create_subset`,
//! which expanded dependencies exactly one level (so `A -> B -> C` silently dropped `C`), and
//! `Channel::component_graph`, a second implementation used only by install-script generation.

use std::collections::{BTreeSet, HashMap};

use crate::{
    manifest::{Channel, Component},
    profile::Profile,
};

/// What the user wants installed for a channel.
///
/// There is exactly one representation. A selection carried forward from a v1 migration is simply
/// one with no profiles and explicit roots, which already yields the intended behaviour -- new
/// dependencies of those roots are picked up, unrelated new profile members are not -- without a
/// second concept.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Intent {
    /// Profiles observed across every activation and direct install for this channel.
    pub profiles: BTreeSet<Profile>,
    /// Components named explicitly.
    pub roots: BTreeSet<String>,
}

impl Intent {
    pub fn new(profiles: &[Profile], roots: &[&str]) -> Self {
        Self {
            profiles: profiles.iter().copied().collect(),
            roots: roots.iter().map(|r| r.to_string()).collect(),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ResolutionError {
    #[error("component '{component}' was requested but does not exist in channel {channel}")]
    UnknownRoot {
        component: String,
        channel: semver::Version,
    },
    #[error("component '{component}' requires '{requires}', which does not exist in the channel")]
    UnknownRequirement { component: String, requires: String },
    #[error("the component graph contains a cycle: {}", path.join(" -> "))]
    RequirementCycle { path: Vec<String> },
    #[error(
        "component '{component}' has kind '{tag}', which is not supported by this version of \
         midenup; upgrade midenup or deselect it"
    )]
    UnsupportedComponentKind { component: String, tag: String },
}

/// Expands `intent` into the exact set of components to install, dependencies first.
///
/// The returned order is a topological sort of the transitive `requires` closure, deduplicated.
pub fn resolve<'a>(
    channel: &'a Channel,
    intent: &Intent,
) -> Result<Vec<&'a Component>, ResolutionError> {
    let index: HashMap<&str, usize> = channel
        .components
        .iter()
        .enumerate()
        .map(|(i, c)| (c.name.as_ref(), i))
        .collect();

    let install_everything = intent.profiles.contains(&Profile::Complete);

    // Roots from profiles. An unsupported component belongs to no profile regardless of what its
    // `profiles` field claims: this build cannot install it, so selecting it implicitly would turn
    // an ordinary `--profile minimal` into a hard failure.
    let mut roots: BTreeSet<usize> = BTreeSet::new();
    for (i, component) in channel.components.iter().enumerate() {
        if !component.is_supported() {
            continue;
        }
        if install_everything || component.profiles.iter().any(|p| intent.profiles.contains(p)) {
            roots.insert(i);
        }
    }

    // Roots named explicitly. Naming an unsupported component *is* an error -- the user asked for
    // something specific and silently omitting it would be the worst outcome.
    for name in intent.roots.iter() {
        let Some(&i) = index.get(name.as_str()) else {
            return Err(ResolutionError::UnknownRoot {
                component: name.clone(),
                channel: channel.name.clone(),
            });
        };
        let component = &channel.components[i];
        if !component.is_supported() {
            return Err(ResolutionError::UnsupportedComponentKind {
                component: component.name.to_string(),
                tag: component.kind().tag().to_string(),
            });
        }
        roots.insert(i);
    }

    // Transitive closure over `requires`. The previous implementation expanded one level only.
    let mut selected: BTreeSet<usize> = BTreeSet::new();
    let mut worklist: Vec<usize> = roots.iter().copied().collect();
    let mut graph = petgraph::graphmap::DiGraphMap::<usize, ()>::new();

    while let Some(i) = worklist.pop() {
        if !selected.insert(i) {
            continue;
        }
        graph.add_node(i);

        let component = &channel.components[i];
        for required in component.requires.iter() {
            let Some(&j) = index.get(required.as_str()) else {
                return Err(ResolutionError::UnknownRequirement {
                    component: component.name.to_string(),
                    requires: required.clone(),
                });
            };

            let dependency = &channel.components[j];
            if !dependency.is_supported() {
                return Err(ResolutionError::UnsupportedComponentKind {
                    component: dependency.name.to_string(),
                    tag: dependency.kind().tag().to_string(),
                });
            }

            graph.add_node(j);
            // Edge points dependency -> dependent, so a topological sort yields dependencies
            // first, which is the order installation needs.
            graph.add_edge(j, i, ());
            worklist.push(j);
        }
    }

    match petgraph::algo::toposort(&graph, None) {
        Ok(order) => Ok(order.into_iter().map(|i| &channel.components[i]).collect()),
        Err(_) => Err(ResolutionError::RequirementCycle { path: find_cycle(&graph, channel) }),
    }
}

/// Names the components on a cycle.
///
/// `toposort` only reports one node in the cycle, which is not enough to act on. The strongly
/// connected component containing it is the cycle, so report all of its members.
fn find_cycle(graph: &petgraph::graphmap::DiGraphMap<usize, ()>, channel: &Channel) -> Vec<String> {
    for scc in petgraph::algo::tarjan_scc(graph) {
        let is_cycle =
            scc.len() > 1 || scc.first().is_some_and(|&n| graph.neighbors(n).any(|m| m == n));
        if is_cycle {
            let mut names: Vec<String> =
                scc.iter().map(|&i| channel.components[i].name.to_string()).collect();
            names.sort();
            return names;
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;
    use crate::{
        artifact::Artifacts,
        manifest::{ComponentKind, OpaqueBody},
        version::Authority,
    };

    /// Builds a channel from `(name, profiles, requires)` triples.
    fn channel(spec: &[(&'static str, &[Profile], &[&str])]) -> Channel {
        channel_with(spec, None)
    }

    /// As [channel], plus an optional component whose kind this build does not recognize.
    fn channel_with(
        spec: &[(&'static str, &[Profile], &[&str])],
        unsupported: Option<(&'static str, &[Profile])>,
    ) -> Channel {
        let mut components: Vec<Component> = spec
            .iter()
            .map(|(name, profiles, requires)| Component {
                name: Cow::Borrowed(name),
                version: Authority::Registry { version: semver::Version::new(0, 1, 0) },
                kind: ComponentKind::Package,
                profiles: profiles.to_vec(),
                requires: requires.iter().map(|r| r.to_string()).collect(),
                artifacts: Artifacts::default(),
                extra: Default::default(),
            })
            .collect();

        if let Some((name, profiles)) = unsupported {
            components.push(Component {
                name: Cow::Borrowed(name),
                version: Authority::Registry { version: semver::Version::new(1, 0, 0) },
                kind: ComponentKind::Unsupported {
                    tag: "wasm-module".to_string(),
                    body: OpaqueBody(Default::default()),
                },
                profiles: profiles.to_vec(),
                requires: vec![],
                artifacts: Artifacts::default(),
                extra: Default::default(),
            });
        }

        Channel::new(semver::Version::new(0, 15, 0), None, components, vec![])
    }

    fn names(resolved: &[&Component]) -> Vec<String> {
        resolved.iter().map(|c| c.name.to_string()).collect()
    }

    /// Regression: `create_subset` expanded one level, so `C` was silently dropped.
    #[test]
    fn closure_is_fully_transitive() {
        let ch =
            channel(&[("a", &[Profile::Minimal], &["b"]), ("b", &[], &["c"]), ("c", &[], &[])]);
        let got = names(&resolve(&ch, &Intent::new(&[Profile::Minimal], &[])).unwrap());
        assert_eq!(got, vec!["c", "b", "a"], "dependencies must precede dependents");
    }

    #[test]
    fn diamond_dependencies_are_deduplicated_and_ordered() {
        let ch = channel(&[
            ("top", &[Profile::Minimal], &["l", "r"]),
            ("l", &[], &["base"]),
            ("r", &[], &["base"]),
            ("base", &[], &[]),
        ]);
        let got = names(&resolve(&ch, &Intent::new(&[Profile::Minimal], &[])).unwrap());

        assert_eq!(got.len(), 4, "no duplicates: {got:?}");
        let pos = |n: &str| got.iter().position(|g| g == n).unwrap();
        assert!(pos("base") < pos("l") && pos("base") < pos("r"));
        assert!(pos("l") < pos("top") && pos("r") < pos("top"));
    }

    #[test]
    fn empty_plus_explicit_root_resolves_to_that_root_and_its_closure() {
        let ch = channel(&[("a", &[Profile::Minimal], &[]), ("b", &[], &["c"]), ("c", &[], &[])]);
        let got = names(&resolve(&ch, &Intent::new(&[Profile::Empty], &["b"])).unwrap());
        assert_eq!(got, vec!["c", "b"], "must not pull in minimal-profile members");
    }

    #[test]
    fn minimal_plus_explicit_root_unions_them() {
        let ch = channel(&[("a", &[Profile::Minimal], &[]), ("b", &[], &[])]);
        let mut got = names(&resolve(&ch, &Intent::new(&[Profile::Minimal], &["b"])).unwrap());
        got.sort();
        assert_eq!(got, vec!["a", "b"]);
    }

    #[test]
    fn complete_selects_every_component_regardless_of_profile_tags() {
        let ch = channel(&[("a", &[], &[]), ("b", &[Profile::Minimal], &[])]);
        assert_eq!(resolve(&ch, &Intent::new(&[Profile::Complete], &[])).unwrap().len(), 2);
    }

    #[test]
    fn complete_dominates_other_profiles_in_the_same_intent() {
        let ch = channel(&[("a", &[], &[]), ("b", &[Profile::Minimal], &[])]);
        let intent = Intent::new(&[Profile::Minimal, Profile::Complete], &[]);
        assert_eq!(resolve(&ch, &intent).unwrap().len(), 2);
    }

    #[test]
    fn an_empty_intent_resolves_to_nothing() {
        let ch = channel(&[("a", &[Profile::Minimal], &[])]);
        assert!(resolve(&ch, &Intent::default()).unwrap().is_empty());
    }

    #[test]
    fn a_missing_explicit_root_is_an_error() {
        let ch = channel(&[("a", &[Profile::Minimal], &[])]);
        assert!(matches!(
            resolve(&ch, &Intent::new(&[], &["nope"])),
            Err(ResolutionError::UnknownRoot { .. })
        ));
    }

    #[test]
    fn a_dangling_requirement_is_an_error() {
        let ch = channel(&[("a", &[Profile::Minimal], &["ghost"])]);
        assert!(matches!(
            resolve(&ch, &Intent::new(&[Profile::Minimal], &[])),
            Err(ResolutionError::UnknownRequirement { .. })
        ));
    }

    #[test]
    fn a_cycle_is_reported_with_its_members() {
        let ch = channel(&[("a", &[Profile::Minimal], &["b"]), ("b", &[], &["a"])]);
        match resolve(&ch, &Intent::new(&[Profile::Minimal], &[])) {
            Err(ResolutionError::RequirementCycle { path }) => {
                assert_eq!(path, vec!["a", "b"], "the cycle members must be named");
            },
            other => panic!("expected a cycle error, got {other:?}"),
        }
    }

    #[test]
    fn a_self_requirement_is_a_cycle() {
        let ch = channel(&[("a", &[Profile::Minimal], &["a"])]);
        assert!(matches!(
            resolve(&ch, &Intent::new(&[Profile::Minimal], &[])),
            Err(ResolutionError::RequirementCycle { .. })
        ));
    }

    /// An unsupported component must never be dragged in by a profile -- that would turn an
    /// ordinary `--profile minimal` into a hard failure on any channel that ships a new kind.
    #[test]
    fn unsupported_components_are_never_implicitly_selected() {
        let ch = channel_with(
            &[("a", &[Profile::Minimal], &[])],
            Some(("futurething", &[Profile::Minimal])),
        );
        let got = names(&resolve(&ch, &Intent::new(&[Profile::Minimal], &[])).unwrap());
        assert_eq!(got, vec!["a"]);

        // `complete` means every component this build can install, not "fail on anything new".
        let got = names(&resolve(&ch, &Intent::new(&[Profile::Complete], &[])).unwrap());
        assert_eq!(got, vec!["a"]);
    }

    /// ...but asking for one by name is an error, not a silent omission.
    #[test]
    fn naming_an_unsupported_component_explicitly_is_an_error() {
        let ch = channel_with(&[("a", &[Profile::Minimal], &[])], Some(("futurething", &[])));
        assert!(matches!(
            resolve(&ch, &Intent::new(&[], &["futurething"])),
            Err(ResolutionError::UnsupportedComponentKind { .. })
        ));
    }

    /// Reaching one through a dependency edge is equally an error: the selected component cannot
    /// work without it.
    #[test]
    fn depending_on_an_unsupported_component_is_an_error() {
        let ch = channel_with(
            &[("a", &[Profile::Minimal], &["futurething"])],
            Some(("futurething", &[])),
        );
        assert!(matches!(
            resolve(&ch, &Intent::new(&[Profile::Minimal], &[])),
            Err(ResolutionError::UnsupportedComponentKind { .. })
        ));
    }
}
