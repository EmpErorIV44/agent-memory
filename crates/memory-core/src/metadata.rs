//! Author-owned Markdown metadata. These fields describe knowledge, never access rights.
use anyhow::{ensure, Context, Result};
use serde::{de::Visitor, Deserialize, Deserializer, Serialize};
use std::{collections::HashSet, fmt};

const MAX_FRONTMATTER_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentMetadata {
    #[serde(
        default,
        rename = "id",
        deserialize_with = "optional_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub stable_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub title: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub created: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub updated: Option<String>,
    #[serde(
        default,
        deserialize_with = "strings",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub tags: Vec<String>,
    #[serde(
        default,
        deserialize_with = "optional_memory",
        skip_serializing_if = "Option::is_none"
    )]
    pub memory: Option<Memory>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Memory {
    #[serde(deserialize_with = "string")]
    pub id: String,
    #[serde(deserialize_with = "string")]
    pub project: String,
    pub kind: MemoryKind,
    pub status: MemoryStatus,
    #[serde(deserialize_with = "string")]
    pub summary: String,
    #[serde(default)]
    pub evidence: Vec<Evidence>,
    #[serde(default)]
    pub relations: Vec<Relation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Fact,
    Decision,
    Implementation,
    Constraint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    Planned,
    Implemented,
    Accepted,
    Superseded,
    Deprecated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub kind: EvidenceKind,
    #[serde(deserialize_with = "string")]
    pub reference: String,
    #[serde(
        default,
        deserialize_with = "optional_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    Document,
    Commit,
    File,
    Test,
    Url,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Relation {
    pub kind: RelationKind,
    #[serde(deserialize_with = "string")]
    pub target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    Supersedes,
    RelatesTo,
    DependsOn,
    Implements,
    Contradicts,
}

/// Validate IDs independently of any transport or storage implementation.
pub fn validate_document_id(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.trim() == value
            && value.len() <= 256
            && !value.chars().any(char::is_control),
        "ID must be a nonempty unpadded string of at most 256 bytes without control characters"
    );
    Ok(())
}

impl Memory {
    pub fn validate(&self) -> Result<()> {
        validate_document_id(&self.id).context("invalid memory.id")?;
        validate_document_id(&self.project).context("invalid memory.project")?;
        ensure!(
            !self.summary.trim().is_empty(),
            "memory.summary must be nonempty"
        );
        for evidence in &self.evidence {
            ensure!(
                !evidence.reference.trim().is_empty()
                    && !evidence.reference.chars().any(char::is_control),
                "evidence.reference must be nonempty and contain no control characters"
            );
            if let Some(description) = &evidence.description {
                ensure!(
                    !description.trim().is_empty(),
                    "evidence.description must be nonempty when present"
                );
            }
        }
        let mut relations = HashSet::new();
        for relation in &self.relations {
            validate_document_id(&relation.target).context("invalid relation.target")?;
            ensure!(relation.target != self.id, "memory cannot relate to itself");
            ensure!(
                relations.insert((relation.kind, &relation.target)),
                "duplicate memory relation"
            );
        }
        Ok(())
    }
}

fn string<'de, D: Deserializer<'de>>(deserializer: D) -> std::result::Result<String, D::Error> {
    struct StringVisitor;
    impl<'de> Visitor<'de> for StringVisitor {
        type Value = String;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a string")
        }

        fn visit_str<E: serde::de::Error>(self, value: &str) -> std::result::Result<String, E> {
            Ok(value.to_owned())
        }

        fn visit_string<E: serde::de::Error>(
            self,
            value: String,
        ) -> std::result::Result<String, E> {
            Ok(value)
        }
    }
    deserializer.deserialize_any(StringVisitor)
}

fn optional_string<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error> {
    string(deserializer).map(Some)
}

fn strings<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<String>, D::Error> {
    struct StrictString(String);
    impl<'de> Deserialize<'de> for StrictString {
        fn deserialize<D: Deserializer<'de>>(
            deserializer: D,
        ) -> std::result::Result<Self, D::Error> {
            string(deserializer).map(Self)
        }
    }
    Vec::<StrictString>::deserialize(deserializer)
        .map(|items| items.into_iter().map(|item| item.0).collect())
}

/// Validate an authored ISO calendar date without inferring filesystem timestamps.
pub fn validate_note_date(value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    ensure!(
        bytes.len() == 10
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes
                .iter()
                .enumerate()
                .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit()),
        "date must use YYYY-MM-DD"
    );
    let year: u32 = value[..4].parse()?;
    let month: u32 = value[5..7].parse()?;
    let day: u32 = value[8..].parse()?;
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 0,
    };
    ensure!(
        year > 0 && day > 0 && day <= days,
        "date must be a valid Gregorian calendar day"
    );
    Ok(())
}

/// Current UTC calendar day for explicit application-managed metadata stamping.
pub fn current_date_utc() -> Result<String> {
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    validate_note_date(&date)?;
    Ok(date)
}

fn optional_memory<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Memory>, D::Error> {
    Memory::deserialize(deserializer).map(Some)
}

/// Return the YAML body and original-byte offset of the Markdown body.
pub(crate) fn frontmatter(text: &str) -> Result<Option<(&str, usize)>> {
    let preamble = if text.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    let mut lines = text[preamble..].split_inclusive('\n');
    let Some(first) = lines.next() else {
        return Ok(None);
    };
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return Ok(None);
    }
    let start = preamble + first.len();
    let mut offset = start;
    for line in lines {
        let end = offset;
        offset += line.len();
        ensure!(
            offset <= MAX_FRONTMATTER_BYTES,
            "YAML frontmatter exceeds 64 KiB"
        );
        if matches!(line.trim_end_matches(['\r', '\n']), "---" | "...") {
            return Ok(Some((&text[start..end], offset)));
        }
    }
    anyhow::bail!("YAML frontmatter is missing its closing delimiter")
}

/// Read document ID and optional logical Memory from YAML; unknown ADR fields are allowed.
pub fn parse_metadata(text: &str) -> Result<DocumentMetadata> {
    let Some((yaml, _)) = frontmatter(text)? else {
        return Ok(DocumentMetadata::default());
    };
    let metadata: DocumentMetadata = if yaml.trim().is_empty() {
        DocumentMetadata::default()
    } else {
        serde_yaml_ng::from_str(yaml).context("invalid YAML frontmatter")?
    };
    if let Some(id) = &metadata.stable_id {
        validate_document_id(id).context("invalid document.id")?;
    }
    if let Some(title) = &metadata.title {
        ensure!(
            !title.trim().is_empty(),
            "document.title must be nonempty when present"
        );
    }
    if let Some(memory) = &metadata.memory {
        memory.validate()?;
    }
    for (name, date) in [
        ("created", &metadata.created),
        ("updated", &metadata.updated),
    ] {
        if let Some(date) = date {
            validate_note_date(date).with_context(|| format!("invalid document.{name}"))?;
        }
    }
    if let (Some(created), Some(updated)) = (&metadata.created, &metadata.updated) {
        ensure!(
            created <= updated,
            "document.created must not be later than document.updated"
        );
    }
    ensure!(
        metadata.tags.len() <= 64,
        "document.tags must contain at most 64 tags"
    );
    let mut tags = HashSet::new();
    for tag in &metadata.tags {
        ensure!(!tag.is_empty() && tag.trim() == tag && tag.len() <= 128 && !tag.chars().any(char::is_control), "tags must be nonempty unpadded strings of at most 128 bytes without control characters");
        ensure!(tags.insert(tag), "duplicate document tag");
    }
    ensure!(
        !(metadata.tags.iter().any(|tag| tag == "recall:always")
            && metadata.tags.iter().any(|tag| tag == "recall:task")),
        "recall:always and recall:task cannot be combined"
    );
    if metadata.tags.iter().any(|tag| tag == "recall:task") {
        ensure!(
            metadata
                .tags
                .iter()
                .filter_map(|tag| tag.strip_prefix("topic:"))
                .all(|topic| topic.chars().any(char::is_alphanumeric)),
            "topic tags must contain at least one word"
        );
    }
    Ok(metadata)
}

/// Explicitly normalize note metadata for an application-managed write.
/// Low-level writes, restores and rename repairs deliberately do not call this.
/// Unknown YAML values and the Markdown body are preserved; YAML formatting may change.
pub fn prepare_note_metadata(content: &str, existing: Option<&str>, today: &str) -> Result<String> {
    validate_note_date(today).context("invalid stamping date")?;
    let metadata = parse_metadata(content)?;
    let old_metadata = existing.map(parse_metadata).transpose()?;
    if existing == Some(content) {
        return Ok(content.to_owned());
    }

    fn parts(content: &str) -> Result<(serde_yaml_ng::Mapping, &str)> {
        if let Some((yaml, offset)) = frontmatter(content)? {
            let value: serde_yaml_ng::Value = if yaml.trim().is_empty() {
                serde_yaml_ng::Value::Mapping(Default::default())
            } else {
                serde_yaml_ng::from_str(yaml)?
            };
            Ok((
                value
                    .as_mapping()
                    .context("frontmatter must be a mapping")?
                    .clone(),
                &content[offset..],
            ))
        } else {
            Ok((
                Default::default(),
                content.strip_prefix('\u{feff}').unwrap_or(content),
            ))
        }
    }
    let (mut fields, body) = parts(content)?;
    let created_key = serde_yaml_ng::Value::String("created".into());
    let updated_key = serde_yaml_ng::Value::String("updated".into());
    if let Some(created) = old_metadata.as_ref().and_then(|m| m.created.as_ref()) {
        ensure!(
            metadata
                .created
                .as_ref()
                .is_none_or(|value| value == created),
            "stamped updates cannot change document.created"
        );
        fields.insert(
            created_key.clone(),
            serde_yaml_ng::Value::String(created.clone()),
        );
    }

    if let Some(existing) = existing {
        let (mut previous, previous_body) = parts(existing)?;
        let mut substantive = fields.clone();
        substantive.remove(&updated_key);
        previous.remove(&updated_key);
        if substantive == previous && body == previous_body {
            return Ok(existing.to_owned());
        }
        fields.insert(updated_key, serde_yaml_ng::Value::String(today.into()));
    } else {
        if metadata.created.is_none() {
            fields.insert(
                created_key,
                serde_yaml_ng::Value::String(metadata.updated.as_deref().unwrap_or(today).into()),
            );
        }
        if metadata.updated.is_none() {
            fields.insert(updated_key, serde_yaml_ng::Value::String(today.into()));
        }
    }
    let newline = if content.starts_with("---\r\n")
        || content.starts_with("\u{feff}---\r\n")
        || (!content.starts_with("---\n") && content.contains("\r\n"))
    {
        "\r\n"
    } else {
        "\n"
    };
    let yaml = serde_yaml_ng::to_string(&fields)?;
    let yaml = if newline == "\r\n" {
        yaml.replace('\n', newline)
    } else {
        yaml
    };
    let bom = if content.starts_with('\u{feff}') {
        "\u{feff}"
    } else {
        ""
    };
    let result = format!("{bom}---{newline}{yaml}---{newline}{body}");
    parse_metadata(&result)
        .context("stamped metadata is inconsistent; provide explicit compatible dates")?;
    Ok(result)
}
