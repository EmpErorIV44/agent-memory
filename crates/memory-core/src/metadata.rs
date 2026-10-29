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
    Ok(metadata)
}
