//! Read-only task context and explicit relation diagnostics. No model or network calls.
use crate::{MemoryRecord, SearchOptions, Store};
use anyhow::{ensure, Context, Result};
use memory_core::{MemoryKind, MemoryStatus, RelationKind};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};

fn active(record: &MemoryRecord) -> bool {
    !matches!(
        record.memory.status,
        MemoryStatus::Superseded | MemoryStatus::Deprecated
    )
}

/// Iterative strongly connected components, avoiding recursion on authored graphs.
fn cycles(records: &[MemoryRecord], kind: RelationKind) -> Vec<Vec<String>> {
    let mut graph = BTreeMap::<String, Vec<String>>::new();
    let mut reverse = BTreeMap::<String, Vec<String>>::new();
    for record in records {
        graph.entry(record.memory.id.clone()).or_default();
        for edge in &record.memory.relations {
            if edge.kind == kind {
                graph
                    .entry(record.memory.id.clone())
                    .or_default()
                    .push(edge.target.clone());
                reverse
                    .entry(edge.target.clone())
                    .or_default()
                    .push(record.memory.id.clone());
            }
        }
    }
    let mut seen = HashSet::new();
    let mut order = Vec::new();
    for root in graph.keys() {
        let mut stack = vec![(root.clone(), false)];
        while let Some((node, finished)) = stack.pop() {
            if finished {
                order.push(node);
                continue;
            }
            if !seen.insert(node.clone()) {
                continue;
            }
            stack.push((node.clone(), true));
            for next in graph.get(&node).into_iter().flatten().rev() {
                if !seen.contains(next) {
                    stack.push((next.clone(), false));
                }
            }
        }
    }
    seen.clear();
    let mut result = Vec::new();
    for root in order.into_iter().rev() {
        if seen.contains(&root) {
            continue;
        }
        let mut component = Vec::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            if !seen.insert(node.clone()) {
                continue;
            }
            component.push(node.clone());
            stack.extend(reverse.get(&node).into_iter().flatten().cloned());
        }
        component.sort();
        if component.len() > 1 {
            result.push(component);
        }
    }
    result.sort();
    result
}

fn diagnostics(records: &[MemoryRecord]) -> Value {
    let by_id: BTreeMap<_, _> = records.iter().map(|r| (r.memory.id.as_str(), r)).collect();
    let mut pairs = BTreeSet::new();
    for record in records.iter().filter(|r| active(r)) {
        for relation in &record.memory.relations {
            let Some(target) = by_id.get(relation.target.as_str()).filter(|r| active(r)) else {
                continue;
            };
            match relation.kind {
                RelationKind::Contradicts => {
                    let mut ids = [record.memory.id.clone(), target.memory.id.clone()];
                    ids.sort();
                    pairs.insert(("active_contradiction", ids[0].clone(), ids[1].clone()));
                }
                RelationKind::Supersedes => {
                    pairs.insert((
                        "active_superseded_target",
                        record.memory.id.clone(),
                        target.memory.id.clone(),
                    ));
                }
                _ => {}
            }
        }
    }
    let mut issues: Vec<Value> = pairs
        .into_iter()
        .map(|(kind, from, to)| json!({"kind":kind,"memory_ids":[from,to]}))
        .collect();
    // Follow supersession through historical intermediates too: retiring the middle
    // of a chain must not hide two active endpoints. Keep a shortest witness path.
    for root in records.iter().filter(|r| active(r)) {
        let mut visited = HashSet::from([root.memory.id.clone()]);
        let mut pending = VecDeque::from([vec![root.memory.id.clone()]]);
        while let Some(path) = pending.pop_front() {
            let current = by_id[path.last().expect("path").as_str()];
            for edge in current
                .memory
                .relations
                .iter()
                .filter(|edge| edge.kind == RelationKind::Supersedes)
            {
                if !visited.insert(edge.target.clone()) {
                    continue;
                }
                let mut next = path.clone();
                next.push(edge.target.clone());
                let target = by_id[edge.target.as_str()];
                if next.len() > 2 && active(target) {
                    issues.push(json!({"kind":"active_supersession_chain","memory_ids":[root.memory.id, target.memory.id],"relation_path":next}));
                }
                pending.push_back(next);
            }
        }
    }
    for (relation, kind) in [
        (RelationKind::DependsOn, "dependency_cycle"),
        (RelationKind::Supersedes, "supersession_cycle"),
    ] {
        for ids in cycles(records, relation) {
            issues.push(json!({"kind":kind,"memory_ids":ids}));
        }
    }
    json!({"issues":issues,"issue_count":issues.len()})
}

impl Store {
    /// Diagnose explicit readable relations only; statuses and notes remain untouched.
    pub fn contradiction_diagnostics(&self, project: &str) -> Result<Value> {
        let records = self.list_memories(project)?;
        let mut report = diagnostics(&records);
        report["project"] = json!(project);
        report["scope"] = json!("explicit_relations_only");
        report["records"] = serde_json::to_value(&records)?;
        report["local_only"] = json!(true);
        Ok(report)
    }

    /// Conservative byte-based token budget for the complete serialized JSON response.
    /// Every UTF-8 byte counts as one token; this is not a model-specific tokenizer.
    pub fn task_context(&self, project: &str, task: &str, max_tokens: usize) -> Result<Value> {
        ensure!(
            (512..=32768).contains(&max_tokens),
            "max_tokens must be between 512 and 32768"
        );
        ensure!(
            !task.trim().is_empty() && task.len() <= 8192,
            "task must contain 1..8192 UTF-8 bytes"
        );
        let records = self.list_memories(project)?;
        let by_doc: BTreeMap<_, _> = records.iter().map(|r| (r.document_id, r)).collect();
        // Terms are individually quoted: user text never becomes an FTS expression.
        let terms: BTreeSet<_> = task
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .take(128)
            .map(|s| format!("\"{}\"", s.replace('"', "\"\"")))
            .collect();
        let query = terms.into_iter().collect::<Vec<_>>().join(" OR ");
        let hits = if query.is_empty() {
            Vec::new()
        } else {
            self.search_filtered(project, &query, &SearchOptions::default())?
        };
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        for record in records.iter().filter(|r| {
            r.memory.kind == MemoryKind::Constraint && r.memory.status == MemoryStatus::Accepted
        }) {
            if seen.insert(record.document_id) {
                candidates.push((
                    record.source_id,
                    record.path.clone(),
                    record.content_hash.clone(),
                    "accepted_constraint",
                    None,
                ));
            }
        }
        for hit in hits {
            if by_doc.get(&hit.document_id).is_some_and(|r| !active(r)) {
                continue;
            }
            if seen.insert(hit.document_id) {
                candidates.push((
                    hit.source_id,
                    hit.path,
                    hit.content_hash,
                    "task_match",
                    Some((hit.start_line, hit.end_line)),
                ));
            }
        }
        // One hop in either direction; include only active project-owned records.
        let selected = seen.clone();
        let by_id: BTreeMap<_, _> = records.iter().map(|r| (r.memory.id.as_str(), r)).collect();
        let mut related_ids = BTreeSet::new();
        for record in records.iter().filter(|r| active(r)) {
            for edge in &record.memory.relations {
                if let Some(target) = by_id.get(edge.target.as_str()).filter(|r| active(r)) {
                    if selected.contains(&record.document_id) {
                        related_ids.insert(target.document_id);
                    }
                    if selected.contains(&target.document_id) {
                        related_ids.insert(record.document_id);
                    }
                }
            }
        }
        for id in related_ids {
            let record = by_doc[&id];
            if seen.insert(id) {
                candidates.push((
                    record.source_id,
                    record.path.clone(),
                    record.content_hash.clone(),
                    "related_memory",
                    None,
                ));
            }
        }
        let issue_count = diagnostics(&records)["issue_count"].clone();
        let mut report = json!({"project":project,"max_tokens":max_tokens,"budget_method":"utf8_bytes_upper_bound","estimated_tokens":0,"local_only":true,"warnings":["untrusted_source_content","evidence_not_verified","remote_freshness_not_checked"],"diagnostic_issue_count":issue_count,"omitted_count":candidates.len(),"entries":[]});
        // Reserve enough space for final counters, even for an empty context.
        ensure!(
            serde_json::to_vec(&report)?.len() + 32 <= max_tokens,
            "budget too small for response metadata"
        );
        let mut omitted = candidates.len();
        for (source_id, path, hash, reason, lines) in candidates {
            let document = self
                .get_document_by_path(project, source_id, &path, &hash)?
                .context("context document changed; retry after refreshing index")?;
            let metadata = memory_core::parse_metadata(&document.content)?;
            if metadata.memory.as_ref().is_some_and(|memory| {
                memory.project != project
                    || matches!(
                        memory.status,
                        MemoryStatus::Deprecated | MemoryStatus::Superseded
                    )
            }) {
                continue;
            }
            let content = if let Some((start, end)) = lines {
                document
                    .content
                    .lines()
                    .skip(start.saturating_sub(1))
                    .take(end.saturating_sub(start) + 1)
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                document.content.clone()
            };
            let mut end = content.len().min(2400);
            while !content.is_char_boundary(end) {
                end -= 1;
            }
            let record = records
                .iter()
                .find(|r| r.source_id == source_id && r.path == path);
            let entry = json!({"source_id":source_id,"path":path,"content_hash":hash,"reason":reason,"memory":record.map(|r| &r.memory),"excerpt":&content[..end],"excerpt_truncated":end < content.len(),"lines":lines});
            report["entries"]
                .as_array_mut()
                .expect("entries")
                .push(entry);
            // Reduce the excerpt before dropping a valuable constraint or source.
            // JSON escaping can expand text, so measure serialized bytes each time.
            while serde_json::to_vec(&report)?.len() + 32 > max_tokens && end > 0 {
                end /= 2;
                while !content.is_char_boundary(end) {
                    end -= 1;
                }
                let entry = report["entries"]
                    .as_array_mut()
                    .expect("entries")
                    .last_mut()
                    .expect("entry");
                entry["excerpt"] = json!(&content[..end]);
                entry["excerpt_truncated"] = json!(end < content.len());
            }
            if serde_json::to_vec(&report)?.len() + 32 > max_tokens {
                report["entries"].as_array_mut().expect("entries").pop();
            } else {
                omitted -= 1;
            }
        }
        report["omitted_count"] = json!(omitted);
        // Fixed point includes the digits of the estimate itself.
        loop {
            let size = serde_json::to_vec(&report)?.len();
            if report["estimated_tokens"] == json!(size) {
                break;
            }
            report["estimated_tokens"] = json!(size);
        }
        Ok(report)
    }
}
