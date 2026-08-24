use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::bail_hint;
use crate::config::LspSettings;
use crate::lsp::acquire;

const MISSING_CACHE_HINT: &str = "run `biskit-mcp doctor` once, or start Biskit outside \
                                  memory-only mode, so the Roblox type definitions and API \
                                  documentation are downloaded into the language server cache";

const SUGGESTIONS: usize = 8;

const DEFAULT_MAX_MEMBERS: usize = 200;

const ENUM_CONTAINER_SUFFIX: &str = "_INTERNAL";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MemberKind {
    Property,
    Method,
    Event,
    Callback,
}

#[derive(Debug, Clone)]
pub struct ApiMember {
    pub name: String,
    pub kind: MemberKind,
    /// The declaration exactly as the type definitions write it.
    pub declaration: String,
    pub deprecated: bool,
    /// What the deprecation says to use instead, where it says.
    pub deprecated_use: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ApiType {
    pub name: String,
    pub extends: Option<String>,
    pub members: Vec<ApiMember>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawParameter {
    #[serde(default)]
    name: String,
    #[serde(default)]
    documentation: String,
}

#[derive(Debug, Clone, Deserialize)]
struct RawDoc {
    #[serde(default)]
    documentation: String,
    #[serde(default)]
    learn_more_link: Option<String>,
    #[serde(default)]
    params: Vec<RawParameter>,
    #[serde(default)]
    returns: Vec<String>,
}

/// The Roblox API surface Biskit already had on disk and never showed anyone.
#[derive(Debug)]
pub struct RobloxApi {
    types: BTreeMap<String, ApiType>,
    services: BTreeSet<String>,
    creatable: BTreeSet<String>,
    docs: HashMap<String, RawDoc>,
    security_level: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemberSummary {
    pub name: String,
    pub kind: MemberKind,
    pub declaration: String,
    #[serde(skip_serializing_if = "crate::json::is_false")]
    pub deprecated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated_use: Option<String>,
    /// Set only when the member came from an ancestor rather than from the class asked about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inherited_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ParameterDoc {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClassAnswer {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    /// The whole ancestry, nearest first, so a member missing here can be looked for above.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inherits: Vec<String>,
    #[serde(skip_serializing_if = "crate::json::is_false")]
    pub is_service: bool,
    #[serde(skip_serializing_if = "crate::json::is_false")]
    pub creatable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub learn_more_link: Option<String>,
    pub members: Vec<MemberSummary>,
    /// How many members survived `member_filter`, which is not how many the class has.
    pub returned_count: usize,
    /// Every member the class carries before `member_filter` narrowed them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_member_count: Option<usize>,
    #[serde(skip_serializing_if = "crate::json::is_false")]
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemberAnswer {
    pub class: String,
    pub name: String,
    /// Not named `kind`: that key is already the discriminator of the answer itself, and a second field by the same name would overwrite it once the two are flattened together.
    pub member_kind: MemberKind,
    pub declaration: String,
    #[serde(skip_serializing_if = "crate::json::is_false")]
    pub deprecated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecated_use: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<ParameterDoc>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub returns: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub learn_more_link: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnumItemAnswer {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnumAnswer {
    pub name: String,
    pub items: Vec<EnumItemAnswer>,
    pub item_count: usize,
    #[serde(skip_serializing_if = "crate::json::is_false")]
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Answer {
    Class(ClassAnswer),
    Member(MemberAnswer),
    Enum(EnumAnswer),
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiResult {
    #[serde(flatten)]
    pub answer: Answer,
    /// Which `globalTypes` file the answer came from.
    pub security_level: &'static str,
}

#[derive(Debug, Clone)]
pub struct ApiQuery<'a> {
    pub query: &'a str,
    pub member_filter: Option<&'a str>,
    pub include_inherited: bool,
    pub include_documentation: bool,
    pub max_members: usize,
}

impl RobloxApi {
    /// Reads the cached type definitions and documentation dump.
    pub fn load(settings: &LspSettings) -> Result<Self> {
        let root = acquire::install_root(settings)?;
        let security_level = settings.roblox_security_level.as_str();

        let definitions = root.join(format!("globalTypes.{security_level}.d.luau"));
        let documentation = root.join(documentation_file_name(settings));

        if !definitions.is_file() {
            bail_hint!(
                MISSING_CACHE_HINT;
                "no Roblox type definitions in the cache at {}",
                definitions.display()
            );
        }

        let source = std::fs::read_to_string(&definitions)
            .with_context(|| format!("failed to read {}", definitions.display()))?;
        let (types, services, creatable) = parse_definitions(&source);

        Ok(Self {
            types,
            services,
            creatable,
            docs: read_docs(&documentation),
            security_level,
        })
    }

    pub fn answer(&self, query: ApiQuery<'_>) -> Result<ApiResult> {
        let asked = query.query.trim();
        if asked.is_empty() {
            bail_hint!(
                "name a class (\"BasePart\"), a member (\"TweenService:Create\"), or an enum \
                 (\"Enum.EasingStyle\")";
                "query must not be empty"
            );
        }

        if let Some(rest) = asked.strip_prefix("Enum.") {
            return self.enum_answer(rest, &query);
        }
        if let Some(found) = self.types.get(asked) {
            return Ok(self.class_answer(found, &query));
        }
        if let Some((owner, member)) = split_member(asked)
            && let Some(found) = self.types.get(owner)
            && let Some(answer) = self.member_answer(found, member)
        {
            return Ok(answer);
        }

        let suggestions = self.suggest(asked);
        let hint = match suggestions.is_empty() {
            true => "names are case-sensitive and spelled as Roblox spells them, such as \
                     \"BasePart\", \"TweenService:Create\", or \"Enum.EasingStyle\""
                .to_string(),
            false => format!("did you mean: {}", suggestions.join(", ")),
        };
        bail_hint!(hint; "the Roblox API has nothing named {asked}")
    }

    fn class_answer(&self, found: &ApiType, query: &ApiQuery<'_>) -> ApiResult {
        let ancestry = self.ancestry(&found.name);
        let mut members: Vec<MemberSummary> = found
            .members
            .iter()
            .map(|member| self.summarise(member, None, query.include_documentation, &found.name))
            .collect();

        if query.include_inherited {
            for ancestor in &ancestry {
                let Some(parent) = self.types.get(ancestor) else {
                    continue;
                };
                members.extend(parent.members.iter().map(|member| {
                    self.summarise(
                        member,
                        Some(ancestor.clone()),
                        query.include_documentation,
                        ancestor,
                    )
                }));
            }
        }

        let total_member_count = members.len();
        if let Some(filter) = query.member_filter {
            let needle = filter.to_lowercase();
            members.retain(|member| member.name.to_lowercase().contains(&needle));
        }

        let returned_count = members.len();
        let limit = query
            .max_members
            .min(DEFAULT_MAX_MEMBERS.max(query.max_members));
        let truncated = returned_count > limit;
        members.truncate(limit);

        let documentation = self.lookup_docs(&found.name);
        ApiResult {
            answer: Answer::Class(ClassAnswer {
                is_service: self.services.contains(&found.name),
                creatable: self.creatable.contains(&found.name),
                documentation: documentation
                    .filter(|_| query.include_documentation)
                    .map(|doc| strip_markup(&doc.documentation))
                    .filter(|text| !text.is_empty()),
                learn_more_link: documentation.and_then(|doc| doc.learn_more_link.clone()),
                name: found.name.clone(),
                extends: found.extends.clone(),
                inherits: ancestry,
                members,
                returned_count,
                total_member_count: (total_member_count != returned_count)
                    .then_some(total_member_count),
                truncated,
            }),
            security_level: self.security_level,
        }
    }

    fn member_answer(&self, owner: &ApiType, member_name: &str) -> Option<ApiResult> {
        let mut current = Some(owner.name.clone());
        while let Some(name) = current {
            let holder = self.types.get(&name)?;
            if let Some(member) = holder
                .members
                .iter()
                .find(|entry| entry.name == member_name)
            {
                let key = format!("{name}.{member_name}");
                let documentation = self.lookup_docs(&key);
                return Some(ApiResult {
                    answer: Answer::Member(MemberAnswer {
                        class: owner.name.clone(),
                        name: member.name.clone(),
                        member_kind: member.kind,
                        declaration: member.declaration.clone(),
                        deprecated: member.deprecated,
                        deprecated_use: member.deprecated_use.clone(),
                        declared_by: (name != owner.name).then(|| name.clone()),
                        documentation: documentation
                            .map(|doc| strip_markup(&doc.documentation))
                            .filter(|text| !text.is_empty()),
                        parameters: documentation
                            .map(|doc| self.parameter_docs(doc))
                            .unwrap_or_default(),
                        returns: documentation
                            .map(|doc| {
                                doc.returns
                                    .iter()
                                    .filter_map(|key| self.resolve_reference(key))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        learn_more_link: documentation.and_then(|doc| doc.learn_more_link.clone()),
                    }),
                    security_level: self.security_level,
                });
            }
            current = holder.extends.clone();
        }
        None
    }

    fn enum_answer(&self, rest: &str, query: &ApiQuery<'_>) -> Result<ApiResult> {
        let (enum_name, item) = match rest.split_once('.') {
            Some((name, item)) => (name, Some(item)),
            None => (rest, None),
        };

        let container = format!("Enum{enum_name}{ENUM_CONTAINER_SUFFIX}");
        let Some(found) = self.types.get(&container) else {
            let suggestions = self.suggest_enum(enum_name);
            let hint = match suggestions.is_empty() {
                true => "enums are spelled as Roblox spells them, such as \"Enum.EasingStyle\""
                    .to_string(),
                false => format!("did you mean: {}", suggestions.join(", ")),
            };
            bail_hint!(hint; "the Roblox API has no enum named Enum.{enum_name}");
        };

        let item_type = format!("Enum{enum_name}");
        let mut items: Vec<EnumItemAnswer> = found
            .members
            .iter()
            .filter(|member| {
                member.kind == MemberKind::Property && member.declaration.ends_with(&item_type)
            })
            .map(|member| EnumItemAnswer {
                documentation: self
                    .docs
                    .get(&format!("@roblox/enum/{enum_name}.{}", member.name))
                    .filter(|_| query.include_documentation)
                    .map(|doc| strip_markup(&doc.documentation))
                    .filter(|text| !text.is_empty()),
                name: member.name.clone(),
            })
            .collect();

        let filter = item
            .map(str::to_string)
            .or(query.member_filter.map(str::to_string));
        if let Some(filter) = filter {
            let needle = filter.to_lowercase();
            items.retain(|entry| entry.name.to_lowercase().contains(&needle));
        }

        let item_count = items.len();
        let truncated = item_count > query.max_members;
        items.truncate(query.max_members);

        Ok(ApiResult {
            answer: Answer::Enum(EnumAnswer {
                name: format!("Enum.{enum_name}"),
                items,
                item_count,
                truncated,
            }),
            security_level: self.security_level,
        })
    }

    fn summarise(
        &self,
        member: &ApiMember,
        inherited_from: Option<String>,
        include_documentation: bool,
        owner: &str,
    ) -> MemberSummary {
        MemberSummary {
            documentation: include_documentation
                .then(|| self.lookup_docs(&format!("{owner}.{}", member.name)))
                .flatten()
                .map(|doc| strip_markup(&doc.documentation))
                .filter(|text| !text.is_empty()),
            name: member.name.clone(),
            kind: member.kind,
            declaration: member.declaration.clone(),
            deprecated: member.deprecated,
            deprecated_use: member.deprecated_use.clone(),
            inherited_from,
        }
    }

    fn parameter_docs(&self, doc: &RawDoc) -> Vec<ParameterDoc> {
        doc.params
            .iter()
            .filter(|parameter| parameter.name != "self")
            .map(|parameter| ParameterDoc {
                name: parameter.name.clone(),
                documentation: self.resolve_reference(&parameter.documentation),
            })
            .collect()
    }

    fn resolve_reference(&self, key: &str) -> Option<String> {
        if !key.starts_with("@roblox/") {
            return Some(strip_markup(key)).filter(|text| !text.is_empty());
        }
        let found = self.docs.get(key)?;
        Some(strip_markup(&found.documentation)).filter(|text| !text.is_empty())
    }

    fn lookup_docs(&self, name: &str) -> Option<&RawDoc> {
        self.docs
            .get(&format!("@roblox/globaltype/{name}"))
            .or_else(|| self.docs.get(&format!("@roblox/global/{name}")))
    }

    fn ancestry(&self, name: &str) -> Vec<String> {
        let mut chain = Vec::new();
        let mut current = self.types.get(name).and_then(|found| found.extends.clone());
        while let Some(parent) = current {
            if chain.contains(&parent) {
                break;
            }
            current = self
                .types
                .get(&parent)
                .and_then(|found| found.extends.clone());
            chain.push(parent);
        }
        chain
    }

    fn suggest(&self, asked: &str) -> Vec<String> {
        let needle = asked.to_lowercase();
        let base = needle.split(['.', ':']).next().unwrap_or(&needle);
        if base.len() < 3 {
            return Vec::new();
        }

        let mut found: Vec<&String> = self
            .types
            .keys()
            .filter(|name| !name.ends_with(ENUM_CONTAINER_SUFFIX))
            .filter(|name| {
                let lowered = name.to_lowercase();
                lowered.contains(base) || (name.len() >= 3 && base.contains(&lowered))
            })
            .collect();

        found.sort_by_key(|name| (name.len(), (*name).clone()));
        found.into_iter().take(SUGGESTIONS).cloned().collect()
    }

    fn suggest_enum(&self, asked: &str) -> Vec<String> {
        let needle = asked.to_lowercase();
        self.types
            .keys()
            .filter_map(|name| name.strip_suffix(ENUM_CONTAINER_SUFFIX))
            .filter_map(|name| name.strip_prefix("Enum"))
            .filter(|name| name.to_lowercase().contains(&needle))
            .take(SUGGESTIONS)
            .map(|name| format!("Enum.{name}"))
            .collect()
    }
}

fn documentation_file_name(settings: &LspSettings) -> String {
    settings
        .documentation_url()
        .rsplit('/')
        .next()
        .unwrap_or("api-docs.json")
        .to_string()
}

fn read_docs(path: &PathBuf) -> HashMap<String, RawDoc> {
    let Ok(raw) = std::fs::read(path) else {
        tracing::warn!(
            target: "biskit",
            "no Roblox API documentation at {}; answers will carry signatures without prose",
            path.display()
        );
        return HashMap::new();
    };
    serde_json::from_slice(&raw).unwrap_or_else(|error| {
        tracing::warn!(target: "biskit", "could not parse {}: {error}", path.display());
        HashMap::new()
    })
}

fn split_member(asked: &str) -> Option<(&str, &str)> {
    let separator = asked.rfind([':', '.'])?;
    let (owner, member) = asked.split_at(separator);
    let member = &member[1..];
    (!owner.is_empty() && !member.is_empty()).then_some((owner, member))
}

fn parse_definitions(
    source: &str,
) -> (
    BTreeMap<String, ApiType>,
    BTreeSet<String>,
    BTreeSet<String>,
) {
    let mut types: BTreeMap<String, ApiType> = BTreeMap::new();
    let mut current: Option<ApiType> = None;
    let mut pending: Option<(bool, Option<String>)> = None;
    let mut services = BTreeSet::new();
    let mut creatable = BTreeSet::new();

    for line in source.lines() {
        if let Some(payload) = line.strip_prefix("--#METADATA#") {
            let (found_services, found_creatable) = parse_metadata(payload);
            services = found_services;
            creatable = found_creatable;
            continue;
        }

        if current.is_none() {
            if let Some(header) = parse_header(line) {
                match header.empty {
                    true => {
                        types.insert(
                            header.name.clone(),
                            ApiType {
                                name: header.name,
                                extends: header.extends,
                                members: Vec::new(),
                            },
                        );
                    }
                    false => {
                        current = Some(ApiType {
                            name: header.name,
                            extends: header.extends,
                            members: Vec::new(),
                        })
                    }
                }
            }
            continue;
        }

        if line == "end" {
            if let Some(finished) = current.take() {
                types.insert(finished.name.clone(), finished);
            }
            pending = None;
            continue;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(attribute) = trimmed.strip_prefix('@') {
            pending = parse_attribute(attribute).or(pending);
            continue;
        }

        let (deprecated, deprecated_use) = pending.take().unwrap_or((false, None));
        if let Some(member) = parse_member(trimmed, deprecated, deprecated_use)
            && let Some(owner) = current.as_mut()
        {
            owner.members.push(member);
        }
    }

    if let Some(finished) = current {
        types.insert(finished.name.clone(), finished);
    }
    (types, services, creatable)
}

struct Header {
    name: String,
    extends: Option<String>,
    empty: bool,
}

fn parse_header(line: &str) -> Option<Header> {
    let rest = line
        .strip_prefix("declare extern type ")
        .or_else(|| line.strip_prefix("declare class "))?;

    let empty = rest.ends_with(" with end") || rest.ends_with(" end");
    let body = rest
        .trim_end_matches(" end")
        .trim_end_matches(" with")
        .trim();

    let (name, extends) = match body.split_once(" extends ") {
        Some((name, parent)) => (name.trim(), Some(parent.trim().to_string())),
        None => (body, None),
    };
    (!name.is_empty()).then_some(Header {
        name: name.to_string(),
        extends,
        empty,
    })
}

fn parse_attribute(attribute: &str) -> Option<(bool, Option<String>)> {
    if !attribute.contains("deprecated") {
        return None;
    }
    let replacement = attribute
        .split_once("use = \"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(value, _)| value.to_string());
    Some((true, replacement))
}

fn parse_member(
    trimmed: &str,
    deprecated: bool,
    deprecated_use: Option<String>,
) -> Option<ApiMember> {
    if let Some(rest) = trimmed.strip_prefix("function ") {
        let name = rest.split('(').next()?.trim();
        return (!name.is_empty()).then(|| ApiMember {
            name: name.to_string(),
            kind: MemberKind::Method,
            declaration: trimmed.to_string(),
            deprecated,
            deprecated_use,
        });
    }

    let (name, type_text) = trimmed.split_once(':')?;
    let name = name.trim();
    let type_text = type_text.trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_alphanumeric() || character == '_')
    {
        return None;
    }

    let kind = if type_text.starts_with("RBXScriptSignal") {
        MemberKind::Event
    } else if type_text.starts_with('(') && type_text.contains("->") {
        MemberKind::Callback
    } else {
        MemberKind::Property
    };

    Some(ApiMember {
        name: name.to_string(),
        kind,
        declaration: trimmed.to_string(),
        deprecated,
        deprecated_use,
    })
}

#[derive(Debug, Deserialize)]
struct Metadata {
    #[serde(rename = "SERVICES", default)]
    services: Vec<String>,
    #[serde(rename = "CREATABLE_INSTANCES", default)]
    creatable_instances: Vec<String>,
}

fn parse_metadata(payload: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let Ok(parsed) = serde_json::from_str::<Metadata>(payload) else {
        return (BTreeSet::new(), BTreeSet::new());
    };
    (
        parsed.services.into_iter().collect(),
        parsed.creatable_instances.into_iter().collect(),
    )
}

fn strip_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for character in text.chars() {
        match character {
            '<' => inside = true,
            '>' => inside = false,
            _ if !inside => out.push(character),
            _ => {}
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFINITIONS: &str = r#"--#METADATA#{"CREATABLE_INSTANCES": ["Part"], "SERVICES": ["TweenService"]}
declare extern type Instance extends Object with
	@[deprecated {use = "Instance:Clone"}]
		function clone(self): Instance
	ChildAdded: RBXScriptSignal<Instance>
	Name: string
	function Destroy(self): nil
end

declare extern type TweenService extends Instance with
	function Create(self, instance: Instance, tweenInfo: TweenInfo, propertyTable: { [string]: any }): Tween
end

declare extern type EnumEasingStyle extends EnumItem with end
declare extern type EnumEasingStyle_INTERNAL extends Enum with
	Linear: EnumEasingStyle
	Sine: EnumEasingStyle
	function GetEnumItems(self): { EnumEasingStyle }
end
"#;

    fn api() -> RobloxApi {
        let (types, services, creatable) = parse_definitions(DEFINITIONS);
        RobloxApi {
            types,
            services,
            creatable,
            docs: HashMap::new(),
            security_level: "PluginSecurity",
        }
    }

    fn query(text: &str) -> ApiQuery<'_> {
        ApiQuery {
            query: text,
            member_filter: None,
            include_inherited: false,
            include_documentation: false,
            max_members: 100,
        }
    }

    #[test]
    fn members_are_classified_by_what_they_are() {
        let api = api();
        let ApiResult {
            answer: Answer::Class(class),
            ..
        } = api.answer(query("Instance")).unwrap()
        else {
            panic!("expected a class answer");
        };

        let kinds: Vec<(&str, MemberKind)> = class
            .members
            .iter()
            .map(|member| (member.name.as_str(), member.kind))
            .collect();
        assert!(kinds.contains(&("ChildAdded", MemberKind::Event)));
        assert!(kinds.contains(&("Name", MemberKind::Property)));
        assert!(kinds.contains(&("Destroy", MemberKind::Method)));
        assert_eq!(class.extends.as_deref(), Some("Object"));
    }

    #[test]
    fn a_deprecation_carries_what_replaced_it() {
        let api = api();
        let ApiResult {
            answer: Answer::Member(member),
            ..
        } = api.answer(query("Instance:clone")).unwrap()
        else {
            panic!("expected a member answer");
        };
        assert!(member.deprecated);
        assert_eq!(member.deprecated_use.as_deref(), Some("Instance:Clone"));
    }

    #[test]
    fn only_the_member_under_an_attribute_is_marked_deprecated() {
        let api = api();
        let ApiResult {
            answer: Answer::Member(member),
            ..
        } = api.answer(query("Instance.Name")).unwrap()
        else {
            panic!("expected a member answer");
        };
        assert!(!member.deprecated);
    }

    #[test]
    fn a_member_is_found_on_an_ancestor_and_says_where_it_came_from() {
        let api = api();
        let ApiResult {
            answer: Answer::Member(member),
            ..
        } = api.answer(query("TweenService:Destroy")).unwrap()
        else {
            panic!("expected a member answer");
        };
        assert_eq!(member.class, "TweenService");
        assert_eq!(member.declared_by.as_deref(), Some("Instance"));
    }

    #[test]
    fn a_service_is_reported_as_one() {
        let api = api();
        let ApiResult {
            answer: Answer::Class(class),
            ..
        } = api.answer(query("TweenService")).unwrap()
        else {
            panic!("expected a class answer");
        };
        assert!(class.is_service);
        assert!(!class.creatable);
        assert_eq!(
            class.inherits,
            vec!["Instance".to_string(), "Object".to_string()]
        );
    }

    #[test]
    fn inherited_members_are_off_by_default_and_labelled_when_asked_for() {
        let api = api();
        let ApiResult {
            answer: Answer::Class(class),
            ..
        } = api.answer(query("TweenService")).unwrap()
        else {
            panic!("expected a class answer");
        };
        assert_eq!(class.members.len(), 1);

        let ApiResult {
            answer: Answer::Class(inherited),
            ..
        } = api
            .answer(ApiQuery {
                include_inherited: true,
                ..query("TweenService")
            })
            .unwrap()
        else {
            panic!("expected a class answer");
        };
        let destroy = inherited
            .members
            .iter()
            .find(|member| member.name == "Destroy")
            .expect("Destroy is inherited from Instance");
        assert_eq!(destroy.inherited_from.as_deref(), Some("Instance"));
    }

    #[test]
    fn an_enum_lists_its_items_and_not_its_methods() {
        let api = api();
        let ApiResult {
            answer: Answer::Enum(found),
            ..
        } = api.answer(query("Enum.EasingStyle")).unwrap()
        else {
            panic!("expected an enum answer");
        };
        assert_eq!(found.name, "Enum.EasingStyle");
        let names: Vec<&str> = found.items.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, vec!["Linear", "Sine"]);
    }

    #[test]
    fn naming_one_enum_item_narrows_the_answer_to_it() {
        let api = api();
        let ApiResult {
            answer: Answer::Enum(found),
            ..
        } = api.answer(query("Enum.EasingStyle.Linear")).unwrap()
        else {
            panic!("expected an enum answer");
        };
        assert_eq!(found.items.len(), 1);
        assert_eq!(found.items[0].name, "Linear");
    }

    #[test]
    fn an_unknown_name_suggests_the_ones_that_exist() {
        let api = api();
        let error = api.answer(query("tweenservice")).unwrap_err();
        let rendered = crate::errors::render("query_roblox_api", &error);
        assert!(rendered.contains("nothing named tweenservice"));
        assert!(rendered.contains("TweenService"));
    }

    #[test]
    fn markup_is_stripped_from_documentation() {
        assert_eq!(
            strip_markup("Creates a new <code>Tween</code> for you."),
            "Creates a new Tween for you."
        );
    }
}
