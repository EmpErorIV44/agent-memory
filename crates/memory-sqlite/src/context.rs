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

fn context_terms(text: &str) -> BTreeSet<String> {
    crate::search_metadata::russian(&text.to_lowercase())
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect()
}

fn prefix_boundary(text: &str, limit: usize) -> usize {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    end
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
        self.task_context_excluding(project, task, max_tokens, &HashSet::new())
    }

    /// Assemble general excerpts without rereading documents managed separately by bootstrap.
    pub fn task_context_excluding(
        &self,
        project: &str,
        task: &str,
        max_tokens: usize,
        excluded: &HashSet<(i64, String)>,
    ) -> Result<Value> {
        self.task_context_impl(project, task, max_tokens, excluded, false)
    }

    /// The unchanged budgeted context plus diagnostics outside its byte budget.
    pub fn task_context_explained(
        &self,
        project: &str,
        task: &str,
        max_tokens: usize,
    ) -> Result<Value> {
        self.task_context_impl(project, task, max_tokens, &HashSet::new(), true)
    }

    fn task_context_impl(
        &self,
        project: &str,
        task: &str,
        max_tokens: usize,
        excluded: &HashSet<(i64, String)>,
        explain: bool,
    ) -> Result<Value> {
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
        let mut hits = if query.is_empty() {
            Vec::new()
        } else {
            self.search_filtered(project, &query, &SearchOptions::default())?
        };
        // Context tasks are prose, so prefer broad task coverage before the
        // search API's exact/stem ranking. Keep its stable order for ties.
        let normalized_terms = context_terms(&query);
        hits.sort_by_cached_key(|hit| {
            let words = context_terms(&format!("{} {}", hit.heading, hit.body));
            std::cmp::Reverse(normalized_terms.intersection(&words).count())
        });
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        for record in records.iter().filter(|r| {
            r.memory.kind == MemoryKind::Constraint
                && crate::recall::required(r)
                && !excluded.contains(&(r.source_id, r.path.clone()))
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
        let hit_diagnostics: BTreeMap<_, _> = if explain {
            hits.iter().rev().map(|hit| {
                let words = context_terms(&format!("{} {}", hit.heading, hit.body));
                ((hit.source_id, hit.path.clone()), json!({
                    "matched_normalized_task_terms": normalized_terms.intersection(&words).collect::<Vec<_>>(),
                    "chunk_id": hit.chunk_id, "lines": [hit.start_line, hit.end_line],
                    "match_scope": "selected_chunk_heading_and_body"
                }))
            }).collect()
        } else {
            BTreeMap::new()
        };
        for hit in hits {
            if excluded.contains(&(hit.source_id, hit.path.clone())) {
                continue;
            }
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
        let mut relation_witnesses: BTreeMap<i64, Vec<Value>> = BTreeMap::new();
        for record in records.iter().filter(|r| active(r)) {
            for edge in &record.memory.relations {
                if let Some(target) = by_id.get(edge.target.as_str()).filter(|r| active(r)) {
                    if selected.contains(&record.document_id) {
                        related_ids.insert(target.document_id);
                        if explain {
                            relation_witnesses.entry(target.document_id).or_default().push(json!({"from": record.memory.id, "to": target.memory.id, "kind": edge.kind}));
                        }
                    }
                    if selected.contains(&target.document_id) {
                        related_ids.insert(record.document_id);
                        if explain {
                            relation_witnesses.entry(record.document_id).or_default().push(json!({"from": record.memory.id, "to": target.memory.id, "kind": edge.kind}));
                        }
                    }
                }
            }
        }
        for id in related_ids {
            let record = by_doc[&id];
            if excluded.contains(&(record.source_id, record.path.clone())) {
                continue;
            }
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
        let mut report = json!({"project":project,"max_tokens":max_tokens,"budget_method":"utf8_bytes_upper_bound","estimated_tokens":0,"local_only":true,"warnings":["untrusted_source_content","evidence_not_verified","remote_freshness_not_checked"],"diagnostic_issue_count":issue_count,"omitted_count":candidates.len(),"omitted_accepted_constraints":candidates.iter().filter(|c| c.3 == "accepted_constraint").count(),"entries":[]});
        // Reserve enough space for final counters, even for an empty context.
        ensure!(
            serde_json::to_vec(&report)?.len() + 32 <= max_tokens,
            "budget too small for response metadata"
        );
        // Prepare complete metadata once. Admit short, useful excerpts before
        // allowing any one entry to consume the remaining budget.
        let mut prepared = Vec::new();
        let mut candidate_diagnostics = Vec::new();
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
                // Metadata is already included as structured data. Spend text
                // capacity on Markdown, preserving its original line numbering.
                memory_core::chunk_markdown(&document.content)
                    .first()
                    .map(|chunk| {
                        document
                            .content
                            .lines()
                            .skip(chunk.start_line - 1)
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default()
            };
            let record = records
                .iter()
                .find(|r| r.source_id == source_id && r.path == path);
            let end = prefix_boundary(&content, 160);
            let entry = json!({"source_id":source_id,"path":path,"content_hash":hash,"reason":reason,"memory":record.map(|r| &r.memory),"excerpt":&content[..end],"excerpt_truncated":end < content.len(),"lines":lines});
            if explain {
                candidate_diagnostics.push(json!({
                    "source_id":source_id,"path":path,"reason":reason,
                    "task_match":if reason == "task_match" { hit_diagnostics.get(&(source_id,path.clone())) } else { None },
                    "relation_witnesses":if reason == "related_memory" { record.and_then(|r| relation_witnesses.get(&r.document_id)) } else { None },
                    "available_excerpt_bytes":content.len()
                }));
            }
            prepared.push((entry, content, end));
        }
        let mut order = Vec::new();
        let constraints: Vec<_> = prepared
            .iter()
            .enumerate()
            .filter(|(_, (e, _, _))| e["reason"] == "accepted_constraint")
            .map(|(i, _)| i)
            .collect();
        let matches: Vec<_> = prepared
            .iter()
            .enumerate()
            .filter(|(_, (e, _, _))| e["reason"] == "task_match")
            .map(|(i, _)| i)
            .collect();
        for i in 0..constraints.len().max(matches.len()) {
            if let Some(id) = constraints.get(i) {
                order.push(*id);
            }
            if let Some(id) = matches.get(i) {
                order.push(*id);
            }
        }
        order.extend(
            prepared
                .iter()
                .enumerate()
                .filter(|(_, (e, _, _))| e["reason"] == "related_memory")
                .map(|(i, _)| i),
        );
        let admission_order = order.clone();
        let mut selected = Vec::new();
        for id in order {
            report["entries"]
                .as_array_mut()
                .expect("entries")
                .push(prepared[id].0.clone());
            if serde_json::to_vec(&report)?.len() + 32 <= max_tokens {
                selected.push(id);
            } else {
                report["entries"].as_array_mut().expect("entries").pop();
            }
        }
        // Keep the established presentation order: constraints, task matches,
        // then relations. Admission order alone alternates the first two groups.
        selected.sort_unstable();
        report["entries"] = json!(selected
            .iter()
            .map(|&id| &prepared[id].0)
            .collect::<Vec<_>>());
        loop {
            let mut grew = false;
            for (position, &id) in selected.iter().enumerate() {
                let (_, content, end) = &mut prepared[id];
                let next = prefix_boundary(content, (*end + 128).min(2400));
                if next <= *end {
                    continue;
                }
                let previous = report["entries"][position].clone();
                report["entries"][position]["excerpt"] = json!(&content[..next]);
                report["entries"][position]["excerpt_truncated"] = json!(next < content.len());
                if serde_json::to_vec(&report)?.len() + 32 <= max_tokens {
                    *end = next;
                    grew = true;
                } else {
                    report["entries"][position] = previous;
                }
            }
            if !grew {
                break;
            }
        }
        let omitted = seen.len() - selected.len();
        report["omitted_count"] = json!(omitted);
        report["omitted_accepted_constraints"] = json!(
            constraints.len()
                - selected
                    .iter()
                    .filter(|&&id| prepared[id].0["reason"] == "accepted_constraint")
                    .count()
        );
        // Fixed point includes the digits of the estimate itself.
        loop {
            let size = serde_json::to_vec(&report)?.len();
            if report["estimated_tokens"] == json!(size) {
                break;
            }
            report["estimated_tokens"] = json!(size);
        }
        if !explain {
            return Ok(report);
        }
        let mut groups = BTreeMap::<String, usize>::new();
        let entries = report["entries"].as_array().expect("entries");
        for entry in entries {
            *groups
                .entry(entry["reason"].as_str().expect("reason").to_owned())
                .or_default() += serde_json::to_vec(entry)?.len();
        }
        let excerpt_json_bytes: usize = entries
            .iter()
            .map(|entry| {
                serde_json::to_vec(&entry["excerpt"])
                    .expect("JSON excerpt")
                    .len()
            })
            .sum();
        let entry_bytes: usize = groups.values().sum();
        let used_bytes = serde_json::to_vec(&report)?.len();
        for (id, candidate) in candidate_diagnostics.iter_mut().enumerate() {
            let position = selected.iter().position(|&selected_id| selected_id == id);
            candidate["admission_order"] =
                json!(admission_order.iter().position(|&i| i == id).map(|p| p + 1));
            candidate["outcome"] = json!(if position.is_some() {
                "included"
            } else {
                "omitted_budget"
            });
            candidate["entry_bytes"] =
                json!(position.map(|p| serde_json::to_vec(&entries[p]).expect("JSON entry").len()));
            candidate["excerpt_bytes"] =
                json!(position.map(|p| entries[p]["excerpt"].as_str().expect("excerpt").len()));
            candidate["excerpt_truncated"] = json!(position
                .map(|p| entries[p]["excerpt_truncated"]
                    .as_bool()
                    .expect("truncated")));
        }
        Ok(json!({"context":report,"explanation":{
            "scope":"prepared_candidates_only", "candidate_limit":50,
            "candidate_limit_scope":"lexical_hits_before_context_resort", "exhaustive":false,
            "query_expression":query,"normalized_task_terms":normalized_terms,
            "admission_policy":"alternate_constraints_and_task_matches_then_relations",
            "growth_policy":"160_initial_bytes_then_128_byte_rounds_up_to_2400",
            "diagnostics_outside_context_budget":true,
            "budget":{"limit_bytes":max_tokens,"used_bytes":used_bytes,"remaining_bytes":max_tokens-used_bytes,
                "entry_bytes_by_reason":groups,"metadata_and_container_bytes":used_bytes-entry_bytes,
                "excerpt_json_bytes":excerpt_json_bytes,"entry_metadata_bytes":entry_bytes-excerpt_json_bytes},
            "candidates":candidate_diagnostics
        }}))
    }
}
