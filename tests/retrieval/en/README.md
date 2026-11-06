# English retrieval corpus

This is a fictional multi-tenant commerce service, not evidence about the actual
Agent Memory implementation. Register `notes/` as a separate source belonging to
project `bench-en`; never register this directory or production knowledge.

The corpus contains 18 notes and 18 frozen cases: thirteen search cases and five
context cases. Cases and expected documents were authored together without
inspecting the retrieval implementation or running queries. Keep those labels
unchanged when reporting the initial measurement. Corrections to genuinely
incorrect labels should be documented separately from retrieval improvements.

`relevant` is a minimum set of documents needed to answer the task, not an
exhaustive set of all defensibly relevant results. Paths are relative to `notes/`.
`forbidden` names documents whose presence violates a particular case; unlisted
results may be distracting but are not automatically incorrect. Search without
status filters may legitimately return historical decisions. Most context cases use a
large explicit budget so baseline accepted constraints are expected along with
current task knowledge. A paired webhook case uses the same relevance labels at
4000 and 16000 tokens to measure lost recall under budget pressure separately; a
small-budget omission is not automatically a contract violation. The no-match
case has an empty relevance set; assess whether the returned set is empty rather
than computing recall with a zero denominator.

The cases cover direct lexical discovery, punctuation-bearing identifiers,
semantic paraphrases, competing authentication lifetimes, current versus retired
decisions, planned feature discovery, baseline constraints and linked knowledge.
Paraphrase cases deliberately use ordinary user language rather than adding
artificial keywords or markers to guarantee a lexical hit. A poor result on such
a case identifies a retrieval limitation; it does not by itself justify a change
of search architecture.
