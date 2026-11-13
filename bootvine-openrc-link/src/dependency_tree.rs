use std::{collections::BTreeMap, convert::Infallible, str::FromStr};

const LINE_PREFIX: &str = "depinfo_";
const SERVICE_FIELD: &str = "service";
const ALL_SERVICES: &str = "*";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ServiceOrdering {
    pub(crate) after: Vec<String>,
    pub(crate) before: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DependencyTree {
    services: BTreeMap<String, ServiceOrdering>,
}

impl DependencyTree {
    pub(crate) fn ordering_of(&self, service: &str) -> ServiceOrdering {
        self.services.get(service).cloned().unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OrderingSide {
    After,
    Before,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CacheField {
    Ordering(OrderingSide),
    Unordered,
}

impl From<&str> for CacheField {
    fn from(field: &str) -> Self {
        match field {
            "ineed" | "iuse" | "iwant" | "iafter" => Self::Ordering(OrderingSide::After),
            "ibefore" => Self::Ordering(OrderingSide::Before),
            _ => Self::Unordered,
        }
    }
}

#[derive(Clone, Copy)]
enum CacheLine<'line> {
    Service {
        index: &'line str,
        name: &'line str,
    },
    Ordering {
        index: &'line str,
        side: OrderingSide,
        other: &'line str,
    },
}

fn unquoted(value: &str) -> &str {
    value.trim().trim_matches('\'')
}

fn cache_line(line: &str) -> Option<CacheLine<'_>> {
    let (key, value) = line.trim().strip_prefix(LINE_PREFIX)?.split_once('=')?;
    let (index, field) = key.split_once('_')?;
    if field == SERVICE_FIELD {
        return Some(CacheLine::Service {
            index,
            name: unquoted(value),
        });
    }
    let (field_name, _) = field.rsplit_once('_')?;
    let CacheField::Ordering(side) = CacheField::from(field_name) else {
        return None;
    };
    let other = unquoted(value);
    (other != ALL_SERVICES).then_some(CacheLine::Ordering { index, side, other })
}

struct NamedIndex<'cache> {
    index: &'cache str,
    name: &'cache str,
}

#[derive(Default)]
struct IndexedServices<'cache> {
    names: Vec<NamedIndex<'cache>>,
    orderings: BTreeMap<&'cache str, ServiceOrdering>,
}

impl<'cache> IndexedServices<'cache> {
    fn record(mut self, line: CacheLine<'cache>) -> Self {
        match line {
            CacheLine::Service { index, name } => self.names.push(NamedIndex { index, name }),
            CacheLine::Ordering { index, side, other } => {
                let ordering = self.orderings.entry(index).or_default();
                match side {
                    OrderingSide::After => ordering.after.push(other.to_owned()),
                    OrderingSide::Before => ordering.before.push(other.to_owned()),
                }
            }
        }
        self
    }
}

impl FromStr for DependencyTree {
    type Err = Infallible;

    fn from_str(cache: &str) -> Result<Self, Infallible> {
        let indexed_services = cache
            .lines()
            .filter_map(cache_line)
            .fold(IndexedServices::default(), IndexedServices::record);
        let IndexedServices {
            names,
            mut orderings,
        } = indexed_services;
        let services = names
            .into_iter()
            .fold(BTreeMap::new(), |mut services, named| {
                services.insert(
                    named.name.to_owned(),
                    orderings.remove(named.index).unwrap_or_default(),
                );
                services
            });
        Ok(Self { services })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CACHE: &str = "\
depinfo_0_service='localmount'
depinfo_0_ineed_0='fsck'
depinfo_0_iuse_0='lvm'
depinfo_0_iafter_0='*'
depinfo_1_service='sshd'
depinfo_1_ineed_0='net'
depinfo_1_ibefore_0='local'
depinfo_1_keyword_0='-docker'
";

    fn parsed() -> DependencyTree {
        let Ok(dependency_tree) = SAMPLE_CACHE.parse();
        dependency_tree
    }

    #[test]
    fn needed_and_used_services_come_before_the_service() {
        assert_eq!(
            parsed().ordering_of("localmount").after,
            vec!["fsck".to_owned(), "lvm".to_owned()]
        );
    }

    #[test]
    fn ordering_before_is_kept_apart_and_keywords_are_ignored() {
        assert_eq!(
            parsed().ordering_of("sshd"),
            ServiceOrdering {
                after: vec!["net".to_owned()],
                before: vec!["local".to_owned()],
            }
        );
    }

    #[test]
    fn unknown_services_have_no_ordering() {
        assert_eq!(parsed().ordering_of("missing"), ServiceOrdering::default());
    }
}
